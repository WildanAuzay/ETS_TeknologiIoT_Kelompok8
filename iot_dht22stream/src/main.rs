use std::{thread, time::Duration};
use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::prelude::*;
use esp_idf_svc::hal::gpio::PinDriver;
use esp_idf_svc::hal::delay::Ets;
use esp_idf_svc::log::EspLogger;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::*;
use esp_idf_svc::mqtt::client::*;
use heapless::String;
use serde_json::json;
use dht_sensor::dht22::Reading;
use dht_sensor::DhtReading;
use esp_idf_svc::systime::EspSystemTime;
use chrono::{DateTime, Utc, NaiveDateTime, Duration as ChronoDuration};

fn main() -> Result<()> {
    // --- Inisialisasi dasar ---
    esp_idf_svc::sys::link_patches();
    EspLogger::initialize_default();
    log::info!("🚀 Program dimulai...");

    // --- Inisialisasi perangkat ---
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    // --- Konfigurasi WiFi ---
    let mut wifi = EspWifi::new(peripherals.modem, sysloop.clone(), Some(nvs))?;
    let mut ssid: String<32> = String::new();
    ssid.push_str("SSID_WIFI").unwrap();

    let mut pass: String<64> = String::new();
    pass.push_str("PASS_WIFI").unwrap();

    let wifi_config = Configuration::Client(ClientConfiguration {
        ssid,
        password: pass,
        auth_method: AuthMethod::WPA2Personal,
        ..Default::default()
    });

    wifi.set_configuration(&wifi_config)?;
    wifi.start()?;
    wifi.connect()?;

    // --- Tunggu sampai WiFi benar-benar aktif ---
    loop {
        if let Ok(status) = wifi.sta_netif().get_ip_info() {
            if status.ip != std::net::Ipv4Addr::UNSPECIFIED {
                log::info!("✅ WiFi terhubung, IP: {:?}", status.ip);
                break;
            }
        }
        log::info!("⏳ Menunggu koneksi WiFi...");
        thread::sleep(Duration::from_secs(1));
    }

    // --- Sinkronisasi waktu via NTP ---
    use esp_idf_svc::sntp;
    use esp_idf_svc::sntp::SyncStatus;

    log::info!("🌐 Sinkronisasi waktu NTP...");
    let sntp = sntp::EspSntp::new_default()?;

    loop {
        let status = sntp.get_sync_status();
        if status == SyncStatus::Completed {
            log::info!("✅ Waktu berhasil disinkronkan dari NTP");
            break;
        } else {
            log::info!("⏳ Menunggu sinkronisasi NTP...");
            thread::sleep(Duration::from_secs(1));
        }
    }

    // Delay tambahan agar waktu stabil
    thread::sleep(Duration::from_secs(5));

    // --- Cek waktu saat ini (WIB) ---
    let systime = EspSystemTime{}.now();
    let secs = systime.as_secs() as i64;
    let nanos = systime.subsec_nanos();
    let naive = NaiveDateTime::from_timestamp_opt(secs, nanos as u32).unwrap();
    let utc_time = DateTime::<Utc>::from_utc(naive, Utc);
    let wib_time = utc_time + ChronoDuration::hours(7);
    log::info!("🕒 Waktu saat ini (WIB): {}", wib_time);

    // --- Konfigurasi MQTT (ThingsBoard Cloud) ---
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("esp32-rust"),
        username: Some("n8gVHc6iooRCXZ0doWID"), // Token Device dari ThingsBoard
        password: None,
        ..Default::default()
    };

    let (mut client, mut connection) =
        EspMqttClient::new("mqtt://mqtt.thingsboard.cloud:1883", &mqtt_config)?;

    log::info!("📡 MQTT client dibuat, menunggu event...");

    // Thread listener event MQTT
    std::thread::spawn(move || {
        while let Ok(event) = connection.next() {
            log::info!("📥 Event MQTT: {:?}", event.payload());
        }
    });

    // --- Inisialisasi sensor DHT22 ---
    let mut pin = PinDriver::input_output_od(peripherals.pins.gpio4)?;
    let mut delay = Ets;

    // --- Loop utama kirim data ---
    loop {
        // Ambil waktu sekarang
        let systime = EspSystemTime{}.now();
        let secs = systime.as_secs() as i64;
        let nanos = systime.subsec_nanos();
        let naive = NaiveDateTime::from_timestamp_opt(secs, nanos as u32).unwrap();
        let utc_time = DateTime::<Utc>::from_utc(naive, Utc);
        let wib_time = utc_time + ChronoDuration::hours(7);

        let ts_millis = naive.timestamp_millis();
        let send_time_str = wib_time.format("%Y-%m-%d %H:%M:%S").to_string();

        // Baca sensor DHT22
        match Reading::read(&mut delay, &mut pin) {
            Ok(Reading { temperature, relative_humidity }) => {
                // Siapkan payload JSON
                let payload = json!({
                    "send_time": send_time_str,   // waktu kirim dari ESP (WIB)
                    "ts": ts_millis,              // waktu epoch (untuk analisis ThingsBoard)
                    "temperature": temperature,
                    "humidity": relative_humidity
                });

                let payload_str = payload.to_string();

                // Hitung waktu publish lokal (latensi sisi ESP)
                let start = EspSystemTime{}.now().as_millis();
                let result = client.publish(
                    "v1/devices/me/telemetry",
                    QoS::AtMostOnce,
                    false,
                    payload_str.as_bytes(),
                );
                let end = EspSystemTime{}.now().as_millis();
                let latency_local = end - start;

                match result {
                    Ok(_) => {
                        log::info!("📤 Data terkirim: {}", payload_str);
                        log::info!("⚡ Latency lokal (publish): {} ms", latency_local);
                    }
                    Err(e) => log::error!("❌ Gagal publish ke MQTT: {:?}", e),
                }
            }
            Err(e) => log::error!("⚠️ Gagal baca DHT22: {:?}", e),
        }

        thread::sleep(Duration::from_secs(60));
    }
}

