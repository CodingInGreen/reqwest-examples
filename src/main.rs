#![no_std]
#![no_main]
#![feature(type_alias_impl_trait)]
#![feature(impl_trait_in_assoc_type)]

use embassy_executor::Spawner;
use embassy_net::{
    tcp::TcpSocket,
    Config,
    Ipv4Address,
    Stack,
    StackResources,
};
use embassy_time::{Duration, Timer};
use esp_backtrace as _;
use esp_hal::{
    clock::ClockControl,
    peripherals::Peripherals,
    rng::Rng,
    system::SystemControl,
    timer::{PeriodicTimer},
};
use esp_println::println;
use esp_wifi::{
    initialize,
    wifi::{
        ClientConfiguration,
        Configuration,
        WifiController,
        WifiDevice,
        WifiEvent,
        WifiStaDevice,
        WifiState,
    },
    EspWifiInitFor,
};

use embedded_nal_async::{TcpClientStack, nb::block};
use embedded_tls::TlsConfig;

const SSID: &str = env!("SSID");
const PASSWORD: &str = env!("PASSWORD");

#[esp_hal_embassy::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let peripherals = Peripherals::take();
    let system = SystemControl::new(peripherals.SYSTEM);
    let clocks = ClockControl::max(system.clock_control).freeze();

    let timer = PeriodicTimer::new(
        esp_hal::timer::timg::TimerGroup::new(peripherals.TIMG0, &clocks)
            .timer0
            .into(),
    );

    let init = initialize(
        EspWifiInitFor::Wifi,
        timer,
        Rng::new(peripherals.RNG),
        peripherals.RADIO_CLK,
        &clocks,
    )
    .unwrap();

    let wifi = peripherals.WIFI;
    let (wifi_interface, controller) =
        esp_wifi::wifi::new_with_mode(&init, wifi, WifiStaDevice).unwrap();

    let config = Config::dhcpv4(Default::default());
    let seed = 1234; // Replace with a proper random seed
    let stack = &*mk_static!(
        Stack<WifiDevice<'_, WifiStaDevice>>,
        Stack::new(
            wifi_interface,
            config,
            mk_static!(StackResources<3>, StackResources::<3>::new()),
            seed
        )
    );

    spawner.spawn(connection(controller)).ok();
    spawner.spawn(net_task(stack)).ok();

    let mut buffer = [0; 1024];
    let mut rx_buffer = [0; 4096];
    let mut tx_buffer = [0; 4096];

    loop {
        if stack.is_link_up() {
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    println!("Waiting to get IP address...");
    loop {
        if let Some(config) = stack.config_v4() {
            println!("Got IP: {}", config.address);
            break;
        }
        Timer::after(Duration::from_millis(500)).await;
    }

    let tls_config = TlsConfig::default();
    let server_addr = (Ipv4Address::new(142, 250, 185, 115), 443);

    let mut socket = TcpSocket::new(stack, &mut rx_buffer, &mut tx_buffer);
    socket.set_timeout(Some(Duration::from_secs(10)));

    println!("Connecting...");
    match block!(socket.connect(server_addr)) {
        Ok(_) => println!("Connected!"),
        Err(e) => {
            println!("Connect error: {:?}", e);
            return;
        }
    }

    let request = b"GET / HTTP/1.0\r\nHost: www.google.com\r\n\r\n";
    match block!(socket.write_all(request)) {
        Ok(_) => println!("Request sent!"),
        Err(e) => {
            println!("Write error: {:?}", e);
            return;
        }
    }

    match block!(socket.read(&mut buffer)) {
        Ok(n) => {
            println!("Received: {}", core::str::from_utf8(&buffer[..n]).unwrap());
        }
        Err(e) => {
            println!("Read error: {:?}", e);
        }
    }

    loop {
        Timer::after(Duration::from_millis(3000)).await;
    }
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    println!("Starting connection task...");
    loop {
        if !matches!(controller.is_started(), Ok(true)) {
            let client_config = Configuration::Client(ClientConfiguration {
                ssid: SSID.into(),
                password: PASSWORD.into(),
                ..Default::default()
            });
            controller.set_configuration(&client_config).unwrap();
            controller.start().await.unwrap();
        }

        if let Err(e) = controller.connect().await {
            println!("Failed to connect: {:?}", e);
            Timer::after(Duration::from_millis(5000)).await;
        } else {
            println!("Connected to WiFi!");
        }
    }
}

#[embassy_executor::task]
async fn net_task(stack: &'static Stack<WifiDevice<'static, WifiStaDevice>>) {
    stack.run().await
}