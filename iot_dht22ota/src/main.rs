use std::{
    str,
    sync::{Arc, Mutex, atomic::{AtomicBool, AtomicUsize, Ordering}},
    thread,
    time::Duration,
};
use anyhow::Result;
use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    hal::prelude::*,
    log::EspLogger,
    mqtt::client::*,
    nvs::EspDefaultNvsPartition,
    wifi::*,
};
use esp_idf_svc::sys as sys;
use heapless::String;

// OTA Const
const OTA_REQUEST_TOPIC: &str = "v1/devices/me/attributes/request/1";
const OTA_RESPONSE_TOPIC: &str = "v1/devices/me/attributes/response/1";
const OTA_TELEMETRY_TOPIC: &str = "v1/devices/me/telemetry";

// Attr
const FW_TITLE_ATTR: &str = "fw_title";
const FW_VERSION_ATTR: &str = "fw_version";
const FW_SIZE_ATTR: &str = "fw_size";
const FW_CHECKSUM_ATTR: &str = "fw_checksum";
const FW_CHECKSUM_ALG_ATTR: &str = "fw_checksum_algorithm";
const FW_STATE_ATTR: &str = "fw_state";

// OTA State
static OTA_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static RECEIVED_BYTES: AtomicUsize = AtomicUsize::new(0);
static FW_SIZE: AtomicUsize = AtomicUsize::new(0);

static mut OTA_HANDLE: sys::esp_ota_handle_t = 0;
static mut UPDATE_PARTITION: *const sys::esp_partition_t = core::ptr::null();

const CHUNK_SIZE: usize = 1024;

// Tambahan metadata OTA global
static mut FW_TITLE: Option<String<32>> = None;
static mut FW_VERSION: Option<String<16>> = None;

fn main() -> Result<()> {
    sys::link_patches();
    EspLogger::initialize_default();

    // WiFi connect
    let peripherals = Peripherals::take().unwrap();
    let sysloop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take().unwrap();
    let mut wifi = EspWifi::new(peripherals.modem, sysloop, Some(nvs))?;
    let mut ssid: String<32> = String::new();
    ssid.push_str("SSID_WIFI").unwrap();
    let mut pass: String<64> = String::new();
    pass.push_str("PASS_WIFI").unwrap();
    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid, password: pass, ..Default::default()
    }))?;
    wifi.start()?;
    wifi.connect()?;
    while !wifi.is_connected()? {
        log::info!("⏳ Menunggu koneksi WiFi...");
        thread::sleep(Duration::from_secs(1));
    }
    log::info!("✅ WiFi OK");

    // MQTT config
    let mqtt_config = MqttClientConfiguration {
        client_id: Some("esp32-rust"),
        username: Some("n8gVHc6iooRCXZ0doWID"), // token
        ..Default::default()
    };

    let mqtt_connected = Arc::new(AtomicBool::new(false));
    let mqtt_connected_cb = mqtt_connected.clone();

    let client_holder: Arc<Mutex<Option<EspMqttClient>>> = Arc::new(Mutex::new(None));
    let client_cb = client_holder.clone();

    let mqtt_callback = move |event: EspMqttEvent<'_>| {
        use esp_idf_svc::mqtt::client::EventPayload;
        match event.payload() {
            EventPayload::Connected(_) => {
                log::info!("📡 MQTT connected");
                mqtt_connected_cb.store(true, Ordering::SeqCst);
            }
            EventPayload::Received { topic, data, .. } => {
                let topic_str = topic.unwrap_or("");
                let payload_str = str::from_utf8(data).unwrap_or("");

                if topic_str == OTA_RESPONSE_TOPIC {
                    if let Some(ref mut client) = *client_cb.lock().unwrap() {
                        handle_ota_response(payload_str, client);
                    }
                } else if topic_str.starts_with("v2/fw/response/") {
                    if let Some(ref mut client) = *client_cb.lock().unwrap() {
                        handle_firmware_chunk(data, client);
                    }
                }
            }
            _ => {}
        }
    };

    let client = unsafe {
        EspMqttClient::new_nonstatic_cb(
            "mqtt://mqtt.thingsboard.cloud:1883",
            &mqtt_config,
            mqtt_callback,
        )?
    };
    *client_holder.lock().unwrap() = Some(client);

    // Tunggu connect
    while !mqtt_connected.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(200));
    }

    {
        let mut guard = client_holder.lock().unwrap();
        if let Some(ref mut client) = *guard {
            client.subscribe(OTA_RESPONSE_TOPIC, QoS::AtLeastOnce)?;
            client.subscribe("v2/fw/response/+/chunk/+", QoS::AtLeastOnce)?;
            request_firmware_attributes(client);
        }
    }

    loop {
        thread::sleep(Duration::from_secs(30));
    }
}

// --- Helper functions ---
fn request_firmware_attributes(client: &mut EspMqttClient) {
    let req = r#"{"sharedKeys":"fw_title,fw_version,fw_size,fw_checksum,fw_checksum_algorithm"}"#;
    client.publish(OTA_REQUEST_TOPIC, QoS::AtLeastOnce, false, req.as_bytes()).unwrap();
    log::info!("📡 Requesting OTA attributes...");
}

fn handle_ota_response(payload: &str, client: &mut EspMqttClient) {
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(payload) {
        if let Some(shared) = json.get("shared") {
            let fw_title = shared.get(FW_TITLE_ATTR).and_then(|v| v.as_str()).unwrap_or("ota");
            let fw_version = shared.get(FW_VERSION_ATTR).and_then(|v| v.as_str()).unwrap_or("1.0");
            let fw_size = shared.get(FW_SIZE_ATTR).and_then(|v| v.as_u64());

            if let Some(size) = fw_size {
                unsafe {
                    // Reset OTA handle
                    OTA_HANDLE = 0;

                    // Ambil partition OTA berikutnya
                    UPDATE_PARTITION = sys::esp_ota_get_next_update_partition(core::ptr::null());
                    if UPDATE_PARTITION.is_null() {
                        log::error!("❌ OTA partition not found!");
                        send_fw_state(client, "FAILED");
                        return;
                    }

                    let ret = sys::esp_ota_begin(UPDATE_PARTITION, size as usize, &mut OTA_HANDLE);
                    if ret != sys::ESP_OK {
                        log::error!("❌ esp_ota_begin failed: {:?}", ret);
                        send_fw_state(client, "FAILED");
                        return;
                    }

                    OTA_IN_PROGRESS.store(true, Ordering::SeqCst);
                    FW_SIZE.store(size as usize, Ordering::SeqCst);
                    RECEIVED_BYTES.store(0, Ordering::SeqCst);

                    // Simpan metadata
                    let mut t: String<32> = String::new();
                    t.push_str(fw_title).unwrap();
                    FW_TITLE = Some(t);
                    let mut v: String<16> = String::new();
                    v.push_str(fw_version).unwrap();
                    FW_VERSION = Some(v);

                    send_fw_state(client, "DOWNLOADING");
                    request_chunk(0, client);
                }
            }
        }
    }
}


fn request_chunk(chunk: usize, client: &mut EspMqttClient) {
    let total = FW_SIZE.load(Ordering::SeqCst);
    let written = RECEIVED_BYTES.load(Ordering::SeqCst);
    let remaining = total.saturating_sub(written);
    if remaining == 0 {
        log::warn!("⚠️ Semua byte sudah diterima, tidak perlu request chunk");
        return;
    }

    let expected_size = remaining.min(CHUNK_SIZE);

    let request_id = 1;
    let topic = format!("v2/fw/request/{}/chunk/{}", request_id, chunk);
    let payload = expected_size.to_string();
    client.publish(&topic, QoS::AtLeastOnce, false, payload.as_bytes()).unwrap();
    log::info!("📡 Request chunk {} ({} bytes)", chunk, expected_size);
}

fn handle_firmware_chunk(payload: &[u8], client: &mut EspMqttClient) {
    if !OTA_IN_PROGRESS.load(Ordering::SeqCst) {
        return;
    }

    let total_size = FW_SIZE.load(Ordering::SeqCst);
    let written_before = RECEIVED_BYTES.load(Ordering::SeqCst);
    let remaining = total_size.saturating_sub(written_before);

    if remaining == 0 {
        log::warn!("⚠️ Semua byte sudah diterima, abaikan chunk tambahan");
        return;
    }

    // Tentukan berapa banyak byte yang akan ditulis
    let mut write_len = payload.len().min(remaining);

    // Align ke 4-byte (esp_ota_write butuh multiple of 4 untuk safety)
    let remainder = write_len % 4;
    if remainder != 0 && write_len > remainder {
        write_len -= remainder;
    }

    unsafe {
        let ret = sys::esp_ota_write(OTA_HANDLE, payload.as_ptr() as *const _, write_len);
        if ret != sys::ESP_OK {
            log::error!("❌ OTA write failed: {:?}", ret);
            send_fw_state(client, "FAILED");
            return;
        }
    }

    RECEIVED_BYTES.fetch_add(write_len, Ordering::SeqCst);
    let written = RECEIVED_BYTES.load(Ordering::SeqCst);

    // Jika masih ada sisa yang tidak kelipatan 4
    let tail_len = payload.len().min(remaining).saturating_sub(write_len);
    if tail_len > 0 {
        let tail_start = write_len;
        let tail = &payload[tail_start..tail_start + tail_len];
        unsafe {
            let ret = sys::esp_ota_write(OTA_HANDLE, tail.as_ptr() as *const _, tail.len());
            if ret != sys::ESP_OK {
                log::error!("❌ OTA write failed (tail): {:?}", ret);
                send_fw_state(client, "FAILED");
                return;
            }
        }
        RECEIVED_BYTES.fetch_add(tail_len, Ordering::SeqCst);
    }

    log::info!(
        "📩 wrote {} bytes (total {}/{})",
        write_len + tail_len,
        RECEIVED_BYTES.load(Ordering::SeqCst),
        total_size
    );

    // Cek apakah semua byte sudah diterima
    if RECEIVED_BYTES.load(Ordering::SeqCst) >= total_size {
        unsafe {
            log::info!("✅ Semua chunk ditulis, finalizing OTA...");
            let ret = sys::esp_ota_end(OTA_HANDLE);
            if ret == sys::ESP_OK {
                sys::esp_ota_set_boot_partition(UPDATE_PARTITION);
                send_fw_state(client, "UPDATED");
                log::info!("🔄 Restarting...");
                sys::esp_restart();
            } else {
                log::error!("❌ OTA end failed: {:?}", ret);
                send_fw_state(client, "FAILED");
            }
        }
    } else {
        // Minta chunk berikutnya
        let next_chunk = RECEIVED_BYTES.load(Ordering::SeqCst) / CHUNK_SIZE;
        request_chunk(next_chunk, client);
    }
}



fn send_fw_state(client: &mut EspMqttClient, state: &str) {
    let msg = format!(r#"{{"{}":"{}"}}"#, FW_STATE_ATTR, state);
    client.publish(OTA_TELEMETRY_TOPIC, QoS::AtLeastOnce, false, msg.as_bytes()).unwrap();
}

