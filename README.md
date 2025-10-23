# 🛰️ IoT DHT22 (Rust + ESP-IDF, ESP32-S3)

Proyek ini terdiri dari dua modul utama yang ditulis dalam **Rust** dan dijalankan di **ESP32-S3**:

1. **IOT_DHT22STREAM** — membaca sensor **DHT22** dan mengirim data suhu & kelembapan ke **ThingsBoard** melalui **MQTT**.  
2. **IOT_DHT22OTA** — versi dengan dukungan **Over-The-Air (OTA)** update menggunakan partisi dual-app.

---

## 📂 Struktur Folder

IOT_DHT22OTA/
├─ .cargo/
│ └─ config.toml
├─ src/
│ └─ main.rs
├─ build.rs
├─ Cargo.toml
├─ partition_table.csv
├─ rust-toolchain.toml
└─ sdkconfig.defaults

IOT_DHT22STREAM/
├─ .cargo/
│ └─ config.toml
├─ src/
│ └─ main.rs
├─ build.rs
├─ Cargo.toml
├─ OTA2.bin
├─ rust-toolchain.toml
└─ sdkconfig.defaults


Kedua proyek dapat dibuild dan dijalankan secara terpisah.

---

## ⚙️ Persiapan & Instalasi

### 1️⃣ Instal Rust
```bash
sudo apt update
sudo apt install curl -y
curl https://sh.rustup.rs -sSf | sh
source $HOME/.cargo/env

# Verifikasi instalasi
rustc --version
cargo --version
```
### 2️⃣ Clone Repositori
```bash
git clone https://github.com/<username-kamu>/IoT-DHT22.git
cd IoT-DHT22
```
### 3️⃣ Siapkan ESP-IDF
```bash
sudo apt install -y git wget flex bison gperf python3 python3-pip cmake ninja-build ccache libffi-dev libssl-dev dfu-util libusb-1.0-0
git clone -b v5.1.1 --recursive https://github.com/espressif/esp-idf.git
cd esp-idf
./install.sh
. ./export.sh
```

#### 🚀 Menjalankan Proyek

Edit kredensial di src/main.rs:
```bash
const WIFI_SSID: &str = "SSID";
const WIFI_PASS: &str = "PASSWORD";
const TB_HOST: &str = "thingsboard.cloud";
const TB_PORT: u16 = 1883;
const TB_TOKEN: &str = "ACCESS_TOKEN";
```
  
    Topik MQTT: v1/devices/me/telemetry
    Contoh payload:
```bash
    {"temperature": 25.7, "humidity": 61.2}
```
## 🔹 1) Jalankan Modul STREAM
```bash
cd IOT_DHT22STREAM
cargo build --release --target xtensa-esp32s3-espidf
cargo install espflash --locked
espflash flash /dev/ttyUSB0 target/xtensa-esp32s3-espidf/release/iot_dht22stream
```
## 🔹 2) Jalankan Modul OTA
```bash
cd ../IOT_DHT22OTA
cargo build --release --target xtensa-esp32s3-espidf
espflash flash --partition-table partition_table.csv /dev/ttyUSB0 target/xtensa-esp32s3-espidf/release/iot_dht22ota
```
    Jika muncul error unexpected argument '--partition-table', jalankan:

    espflash flash --help

    dan sesuaikan posisi argumen berdasarkan versi espflash milikmu.

## 🧪 Langkah Percobaan
   - Buat Device baru di ThingsBoard dan salin Access Token.    
   - Hubungkan DHT22 ke pin GPIO sesuai kode (gunakan pull-up 4.7–10kΩ).
   - Isi SSID, Password, dan Token di kode.
   - Build & flash.
   - Buka serial monitor untuk memastikan Wi-Fi & MQTT tersambung.
   - Buka Latest Telemetry di ThingsBoard → data muncul setiap ±10 detik.
   - Untuk OTA: flash firmware awal, ubah versi kode → rebuild → update image baru → reboot otomatis ke partisi baru.

## 🧱 Diagram Sistem

flowchart LR
 ```bash
  DHT22 --> ESP32S3 --> WiFi --> MQTT --> TB[(ThingsBoard Cloud)] --> User
```
## 📊 Hasil & Analisis
- Sensor DHT22 membaca suhu dan kelembapan.
- Data dikirim ke ThingsBoard dalam format JSON dan tampil pada dashboard.
- Perubahan kecil suhu/kelembapan adalah normal (±0.5°C).
- OTA sukses jika device reboot ke firmware baru & telemetry tetap berjalan normal.

## 🛠️ Troubleshooting
Masalah	Solusi
- unwrap() on Err: environment variable not found
  ```bash
  Jalankan . ./export.sh sebelum cargo build
  ```
- Telemetri tidak muncul
```bash
  Periksa Access Token, host, port, dan koneksi Wi-Fi
```
- Nilai sensor aneh
  ```bash
  Periksa pull-up DATA, catu 3.3V stabil, kabel pendek
  ```
- OTA gagal boot
  ```bash
  Pastikan partition_table.csv benar dan image valid
  ```
