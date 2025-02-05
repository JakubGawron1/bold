use crate::arch::aarch64::mmio::{
    delay, mmio_read, mmio_write, GPFSEL1, GPPUD, GPPUDCLK0, UART1_ENABLE, UART1_MU_BAUD,
    UART1_MU_CNTL, UART1_MU_IER, UART1_MU_IIR, UART1_MU_IO, UART1_MU_LCR, UART1_MU_LSR,
    UART1_MU_MCR,
};
use crate::driver_manager::{DeviceType, DriverInfo};
use crate::ipc;
use crate::prelude::*;

use crate::{driver_manager, fi, ktask};
use core::cell::UnsafeCell;
use spin::RwLock;

// ----- Driver -----

#[derive(Debug)]
struct Driver {
    info: UnsafeCell<DriverInfo>,
}

pub fn init_uart1() {
    unsafe {
        // Initialize UART
        mmio_write(UART1_ENABLE, mmio_read(UART1_ENABLE) | 1);
        mmio_write(UART1_MU_CNTL, 0);
        mmio_write(UART1_MU_LCR, 3); // 8 bits
        mmio_write(UART1_MU_MCR, 0);
        mmio_write(UART1_MU_IER, 0);
        mmio_write(UART1_MU_IIR, 0xc6); // disable interrupts
        mmio_write(UART1_MU_BAUD, 270); // 115200 baud

        // Map UART1 to GPIO pins
        mmio_write(GPFSEL1, {
            let mut new_val = mmio_read(GPFSEL1);
            new_val &= !((7 << 12) | (7 << 15)); // gpio14, gpio15
            new_val |= (2 << 12) | (2 << 15); // alt5
            new_val
        });
        mmio_write(GPPUD, 0); // enable pins 14 and 15
        delay(1500);
        mmio_write(GPPUDCLK0, (1 << 14) | (1 << 15));
        delay(1500);
        mmio_write(GPPUDCLK0, 0); // flush GPIO setup
        mmio_write(UART1_MU_CNTL, 3); // enable Tx, Rx
    }
}

pub fn write_uart1(buf: &[u8]) -> IoResult<usize> {
    for c in buf {
        unsafe {
            // Wait for UART to become ready to transmit.
            while mmio_read(UART1_MU_LSR) & 0x20 == 0 {}
            mmio_write(UART1_MU_IO, *c as u32);
        }
    }
    Ok(buf.len())
}

impl driver_manager::Driver for Driver {
    fn init(&self) -> Result<(), ()> {
        // FIXME: Vulnerability
        unsafe {
            (*self.info.get()).initialized = true;
        }

        spawn_task!(b"UART1.input", {
            // Create the input queue
            let root = ipc::ROOT.read().as_ref().unwrap().clone();
            let input_queue = root
                .dir_get(ipc::well_known::ROOT_DEVICES)
                .await
                .unwrap()
                .dir_get(ipc::well_known::DEVICES_RPI_UART)
                .await
                .unwrap()
                .dir_get(ipc::well_known::RPI_UART1)
                .await
                .unwrap()
                .dir_link(ipc::well_known::RPI_UART_IN, ipc::IpcSpscQueue::new())
                .await
                .unwrap();

            // Write to it forever
            let mut buf = [0u8; 1];
            loop {
                if let Ok(1) = fi::Read::read(&DEVICE, &mut buf).await {
                    input_queue.queue_write(&buf).warn();
                }
                ktask::yield_now().await;
            }
        });

        spawn_task!(b"UART1.output", {
            // Create the output queue
            let root = ipc::ROOT.read().as_ref().unwrap().clone();
            let output_queue = root
                .dir_get(ipc::well_known::ROOT_DEVICES)
                .await
                .unwrap()
                .dir_get(ipc::well_known::DEVICES_RPI_UART)
                .await
                .unwrap()
                .dir_get(ipc::well_known::RPI_UART1)
                .await
                .unwrap()
                .dir_link(ipc::well_known::RPI_UART_OUT, ipc::IpcSpscQueue::new())
                .await
                .unwrap();

            // Write to it forever
            let mut buf = [0u8; 512];
            loop {
                if let Some(count) = output_queue.queue_read(&mut buf).await {
                    if count != 0 {
                        // fi::SyncWrite::write_all(&DEVICE, &buf).await.unwrap();
                        fi::SyncWrite::write_all(&DEVICE, &buf[0..count]).unwrap();
                    }
                }
                ktask::yield_now().await;
            }
        });

        Ok(())
    }

    fn info(&'static self) -> &'static DriverInfo {
        // FIXME: Vulnerability
        unsafe { self.info.get().as_ref().unwrap() }
    }
}

static mut DRIVER: Driver = Driver {
    info: UnsafeCell::new(DriverInfo {
        name: b"Raspberry Pi 3 UART1",
        initialized: false,
        devices: RwLock::new([driver_manager::Device {
            device_type: DeviceType::Console,
            interface: fi::FileInterface {
                sync_read: Some(&DEVICE),
                read: Some(&DEVICE),
                sync_write: Some(&DEVICE),
                write: Some(&DEVICE),
                ctrl: None,
            },
        }]),
    }),
};

#[link_section = ".drivers"]
#[used]
static mut DRIVER_REF: &dyn driver_manager::Driver = unsafe { &DRIVER };

// ----- Device -----

#[derive(Debug)]
struct Device;

#[async_trait]
impl fi::Write for Device {
    async fn write(&self, buf: &[u8]) -> IoResult<usize> {
        for c in buf {
            unsafe {
                // Wait for UART to become ready to transmit.
                while mmio_read(UART1_MU_LSR) & 0x20 == 0 {
                    ktask::yield_now().await;
                }
                mmio_write(UART1_MU_IO, *c as u32);
            }
        }
        Ok(buf.len())
    }
}

#[async_trait]
impl fi::Read for Device {
    async fn read(&self, buf: &mut [u8]) -> IoResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        unsafe {
            // Wait for UART to become ready to receive.
            while mmio_read(UART1_MU_LSR) & 0x1 == 0 {
                // Poll UART1 at 120hz (1000×1000÷120 = 8333us)
                wtfln!("(UART)");
                crate::arch::aarch64::mmio::sleep_us(8333).await;
                // crate::arch::aarch64::mmio::sleep_us(1010).await;
                // yield_now().await;
            }
            buf[0] = mmio_read(UART1_MU_IO) as u8;
        }
        Ok(1)
    }
}

impl fi::SyncWrite for Device {
    fn write(&self, buf: &[u8]) -> IoResult<usize> {
        for c in buf {
            unsafe {
                // Wait for UART to become ready to transmit.
                while mmio_read(UART1_MU_LSR) & 0x20 == 0 {}
                mmio_write(UART1_MU_IO, *c as u32);
            }
        }
        Ok(buf.len())
    }
}

impl fi::SyncRead for Device {
    fn read(&self, buf: &mut [u8]) -> IoResult<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        unsafe {
            // Wait for UART to become ready to receive.
            while mmio_read(UART1_MU_LSR) & 0x1 == 0 {}
            buf[0] = mmio_read(UART1_MU_IO) as u8;
        }
        Ok(1)
    }
}

static DEVICE: Device = Device;
