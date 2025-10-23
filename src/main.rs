//! This example shows how to communicate asynchronous using i2c with external chips.
//!
//! Example written for the [`MCP23017 16-Bit I2C I/O Expander with Serial Interface`] chip.
//! (https://www.microchip.com/en-us/product/mcp23017)

#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]
use core::panic::PanicInfo;

use cortex_m::asm::delay;
#[panic_handler]
pub fn panic(info: &PanicInfo) -> ! {
    loop {
        log::info!("{}", info);
        delay(1000000);
    }
}

use defmt_rtt as _;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::i2c::{self, Config, InterruptHandler};
use embassy_rp::peripherals::I2C1;
use embassy_rp::peripherals::USB;
use embassy_rp::*;
use embedded_hal_1::i2c::I2c;

use embassy_time::Timer;
use mcp230xx::*;
use rp2040_project_template::run;

bind_interrupts!(struct Irqs {
    I2C1_IRQ => InterruptHandler<I2C1>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;

});

#[allow(dead_code)]
mod mcp23017 {
    pub const ADDR: u8 = 0x20; // default addr

    macro_rules! mcpregs {
        ($($name:ident : $val:expr),* $(,)?) => {
            $(
                pub const $name: u8 = $val;
            )*

            pub fn regname(reg: u8) -> &'static str {
                match reg {
                    $(
                        $val => stringify!($name),
                    )*
                    _ => panic!("bad reg"),
                }
            }
        }
    }

    // These are correct for IOCON.BANK=0
    mcpregs! {
        IODIRA: 0x00,
        IPOLA: 0x02,
        GPINTENA: 0x04,
        DEFVALA: 0x06,
        INTCONA: 0x08,
        IOCONA: 0x0A,
        GPPUA: 0x0C,
        INTFA: 0x0E,
        INTCAPA: 0x10,
        GPIOA: 0x12,
        OLATA: 0x14,
        IODIRB: 0x01,
        IPOLB: 0x03,
        GPINTENB: 0x05,
        DEFVALB: 0x07,
        INTCONB: 0x09,
        IOCONB: 0x0B,
        GPPUB: 0x0D,
        INTFB: 0x0F,
        INTCAPB: 0x11,
        GPIOB: 0x13,
        OLATB: 0x15,
    }
}

#[embassy_executor::task]
async fn logger_task(driver: usb::Driver<'static, USB>) {
    run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    let driver = usb::Driver::new(p.USB, Irqs);
    spawner.spawn(logger_task(driver)).unwrap();
    let sda = p.PIN_2;
    let scl = p.PIN_3;

    Timer::after_secs(5).await;

    log::info!("set up i2c ");
    let mut i2c = i2c::I2c::new_blocking(p.I2C1, scl, sda, Config::default());

    use mcp23017::*;

    log::info!("init mcp23017 config for IxpandO");
    i2c.write(ADDR, &[IODIRB, 0xff]).unwrap();
    // init - a outputs, b inputs
    // let mut u = Mcp230xx::<I2C, Mcp23017>::default(i2c).unwrap();

    let val = 1;
    loop {
        let mut portb = [0];

        i2c.write_read(mcp23017::ADDR, &[GPIOB], &mut portb)
            .unwrap();
        log::info!("portb = {:08b}", portb[0]);

        // get a register dump
        // log::info!("getting register dump");

        Timer::after_millis(100).await;
    }

    // loop {
    //     log::info!("aaaaa");
    //     Timer::after_secs(1).await;
    // }
}
