//! Blinks the LED on a Pico board
//!
//! This will blink an LED attached to GP25, which is the pin the Pico uses for the on-board LED.
#![no_std]
#![no_main]
#![allow(async_fn_in_trait)]

use core::str::{self, from_utf8};

// use bsp::entry;
use cyw43_pio::PioSpi;
use defmt::*;
use embassy_executor::Spawner;
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::{PacketMetadata, UdpSocket};
use embassy_net::{Config as NetConfig, DhcpConfig, IpEndpoint, Stack, StackResources};
use embassy_rp::clocks::RoscRng;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::USB;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::*;
use embassy_rp::{bind_interrupts, peripherals::DMA_CH0, peripherals::PIO0};
use embassy_time::{Duration, Timer};
use panic_probe as _;
use rand::RngCore;
use rp2040_project_template::run;
use rp2040_project_template::UsbLogger;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

// Provide an alias for our BSP so we can switch targets quickly.
// Uncomment the BSP you included in Cargo.toml, the rest of the code does not need to change.
// use rp_pico as bsp;
// use sparkfun_pro_micro_rp2040 as bsp;

// use bsp::hal::{
//     clocks::{init_clocks_and_plls, Clock},
//     pac,
//     sio::Sio,
//     watchdog::Watchdog,
// };

const WIFI_NETWORK: &str = env!("WIFI_NETWORK"); // change to your network SSID
const WIFI_PASSWORD: &str = env!("WIFI_PASSWORD");

bind_interrupts!(struct Irqs{
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
    USBCTRL_IRQ => usb::InterruptHandler<USB>;
});
#[embassy_executor::task]
async fn cyw43_task(
    runner: cyw43::Runner<'static, Output<'static>, PioSpi<'static, PIO0, 0, DMA_CH0>>,
) -> ! {
    runner.run().await
}

#[embassy_executor::task]
async fn logger_task(driver: usb::Driver<'static, USB>) {
    run!(1024, log::LevelFilter::Info, driver);
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<cyw43::NetDriver<'static>>) -> ! {
    stack.run().await
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    let mut rng = RoscRng;

    let fw = include_bytes!("../assets/43439A0.bin");
    let clm = include_bytes!("../assets/43439A0_clm.bin");

    let pwr = Output::new(p.PIN_23, Level::Low);
    let cs = Output::new(p.PIN_25, Level::High);
    let mut pio = Pio::new(p.PIO0, Irqs);
    let driver = usb::Driver::new(p.USB, Irqs);
    spawner.spawn(logger_task(driver)).unwrap();
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        pio.irq0,
        cs,
        p.PIN_24,
        p.PIN_29,
        p.DMA_CH0,
    );

    static STATE: StaticCell<cyw43::State> = StaticCell::new();
    let state = STATE.init(cyw43::State::new());
    let (net_device, mut control, runner) = cyw43::new(state, pwr, spi, fw).await;
    unwrap!(spawner.spawn(cyw43_task(runner)));
    control.init(clm).await;

    control
        .set_power_management(cyw43::PowerManagementMode::PowerSave)
        .await;

    let delay = Duration::from_millis(300);
    {
        let mut scanner = control.scan(Default::default()).await;
        while let Some(bss) = scanner.next().await {
            if let Ok(ssid_str) = str::from_utf8(&bss.ssid) {
                log::info!("scanned {}", ssid_str);
            }
        }
    }

    let config = NetConfig::dhcpv4(Default::default());

    let seed = rng.next_u64();

    static STACK: StaticCell<Stack<cyw43::NetDriver<'static>>> = StaticCell::new();
    static RESOURCES: StaticCell<StackResources<5>> = StaticCell::new();
    let stack = &*STACK.init(Stack::new(
        net_device,
        config,
        RESOURCES.init(StackResources::new()),
        seed,
    ));
    let mac_addr = stack.hardware_address();
    log::info!("Hardware configured. MAC Address is {}", mac_addr);

    unwrap!(spawner.spawn(net_task(stack)));

    control
        .join_wpa2(WIFI_NETWORK, WIFI_PASSWORD)
        .await
        .unwrap();

    for count in 1..10 {
        if stack.is_config_up() {
            break;
        }
        Timer::after_secs(1).await;
    }

    match stack.config_v4() {
        Some(a) => {
            log::info!("IP: {}", a.address)
        }
        None => {
            log::warn!("Failed GET IP")
        }
    }

    let mut udp_rx_meta = [PacketMetadata::EMPTY; 16];
    let mut udp_rx_buffer = [0; 1024];
    let mut udp_tx_meta = [PacketMetadata::EMPTY; 16];
    let mut udp_tx_buffer = [0; 1024];
    let mut msg_buffer = [0; 128];
    // I'll reuse the earlier msg_buffer since we're done with the TCP part

    let mut udp_socket = UdpSocket::new(
        &stack,
        &mut udp_rx_meta,
        &mut udp_rx_buffer,
        &mut udp_tx_meta,
        &mut udp_tx_buffer,
    );

    udp_socket.bind(8080).unwrap();

    let mut led: bool = false;

    loop {
        if led {
            log::info!("led on!");
            control.gpio_set(0, true).await;
        }

        Timer::after(delay).await;

        while udp_socket.may_recv() {
            log::info!("recv!");
            let (rx_size, from_addr) = match udp_socket.recv_from(&mut msg_buffer).await {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("{:?}", e);
                    continue;
                }
            };
            log::info!("Size: {}", rx_size);
            let response = from_utf8(&msg_buffer[..rx_size]).unwrap();
            log::info!("Server replied with {} from {}", response, from_addr);

            if response.contains("led on") {
                led = true
            } else if response.contains("led off") {
                led = false
            }
        }

        log::info!("led off!");
        control.gpio_set(0, false).await;
        Timer::after(delay).await;
    }
}

// End of file
