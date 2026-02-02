# RFU (Rignite Device Flasher) - Android Implementation Guide

This document outlines the technical requirements and implementation details for the Android application component of the Rignite Device Flasher (RDF). The Android app serves as the "source" of the OS image, streaming it over USB to the Rignite bootloader (running on the target PC).

## 1. Architecture Overview

In this setup, the roles are reversed from typical Android file transfers:
*   **PC (Rignite Bootloader):** Acts as the **USB Host**.
*   **Android Device:** Acts as the **USB Peripheral** (specifically, mimicking a standard USB Mass Storage or a custom Vendor-Specific Class).

**Note:** While Android has a specific "USB Accessory" mode (AOA), that requires the *PC* to have specific drivers to send AOA triggers. Since UEFI USB stacks are minimal, it is often easier to rely on standard USB Bulk Transfer endpoints provided by the Android `UsbManager` in "Device Mode" or by implementing a lightweight userspace driver if the phone supports it.

However, the most robust path for a UEFI host that only speaks simple protocols is for the Android app to interact via the **Android USB Host API** if the *PC* is acting as a peripheral (e.g. via USB-A to USB-A cable, which is dangerous/rare), OR more commonly:

**The Standard Rignite Approach:**
The PC is the Host. The Phone is the Device.
Android exposes itself via the **Android Open Accessory (AOA) Protocol** is *possible* but complex for UEFI.
A simpler alternative for Phase 1 is treating the connection as a raw byte stream if the hardware allows, but standard non-rooted Android blocks low-level USB gadget manipulation.

**Therefore, the recommended approach for the finalized App:**
The Android device should function in **USB Accessory Mode** if Rignite can send the trigger, OR simply rely on **ADB/MTP** if Rignite implements a minimal client.

*For the purpose of this guide, we assume Rignite will implement a basic USB Bulk transfer reader, and the Android App will use the `UsbManager` API to find the connected PC (Host) and send data.*

## 2. Android Manifest Configuration

The app must request permission to use USB devices.

```xml
<manifest ...>
    <uses-feature android:name="android.hardware.usb.host" />
    <uses-permission android:name="android.permission.USB_PERMISSION" />

    <application ...>
        <activity ...>
            ...
            <intent-filter>
                <action android:name="android.hardware.usb.action.USB_DEVICE_ATTACHED" />
            </intent-filter>

            <meta-data
                android:name="android.hardware.usb.action.USB_DEVICE_ATTACHED"
                android:resource="@xml/device_filter" />
        </activity>
    </application>
</manifest>
```

**res/xml/device_filter.xml**
Ideally, we filter for a specific interface class if we can't predict the PC's VID/PID (since PCs act as hosts, they don't usually have a "device" VID/PID unless looking at a specific bridge chip).

*Actually, since the PC is the HOST, the Android phone is the DEVICE. The Android app doesn't "find" the PC in `UsbManager` (which is for when the Phone is Host).*

**Correction for Android Logic:**
When the phone is plugged into a PC:
1.  The **PC (Rignite)** enumerates the Phone.
2.  The Phone appears as a composite device (MTP, ADB, etc.).
3.  **Rignite** must find the specific USB Endpoint to talk to.

To make this seamless without writing a complex MTP driver in UEFI, the Android app should enable **USB Tethering (RNDIS)** or a custom **Accessory Mode**.

**The "User-Space Network" Approach (Recommended for UEFI)**
UEFI often has drivers for USB Networking (RNDIS/CDC-ECM).
1.  **Android:** User enables "USB Tethering".
2.  **Rignite:** Uses `EFI_SIMPLE_NETWORK_PROTOCOL` on the USB interface.
3.  **Android App:** Opens a TCP Socket Server on Port `8080`.
4.  **Rignite:** Connects to `192.168.x.x:8080` and streams the image.

This avoids writing raw USB drivers in UEFI and uses the OS's existing network stack.

---

## 3. Protocol Implementation (TCP/IP Variant)

If using the Network approach (easiest for cross-platform support including iOS):

### 3.1. Discovery
The App starts a TCP server and broadcasts via UDP or simply waits. Rignite assumes the gateway IP is the phone.

### 3.2. The Handshake (RDF Header)
When Rignite connects, the App sends the 128-byte header immediately.

```java
ByteBuffer header = ByteBuffer.allocate(128);
header.order(ByteOrder.LITTLE_ENDIAN);

// Magic: 0x52 0x44 0x46 0x21 ("RDF!")
header.put(new byte[] { 0x52, 0x44, 0x46, 0x21 });

// Image Size (u64)
long imageSize = file.length();
header.putLong(imageSize);

// Checksum (32 bytes - SHA256)
byte[] checksum = calculateSha256(file);
header.put(checksum);

// Target Subvolume (64 bytes, zero-padded string)
byte[] subvol = "@core".getBytes(StandardCharsets.UTF_8);
header.put(subvol);
// ... pad remaining ...

outputStream.write(header.array());
```

### 3.3. Streaming
The app simply streams the file chunks.

```java
byte[] buffer = new byte[64 * 1024]; // 64KB chunks
int len;
while ((len = fileInputStream.read(buffer)) != -1) {
    outputStream.write(buffer, 0, len);
}
```

---

## 4. Protocol Implementation (Raw USB Bulk Variant)

If we stick to the **"Thin Pipe"** architecture (Raw USB), the PC is Host, Phone is Device.
Standard Android does not allow an App to define custom USB Descriptors to the Host (that requires Root/ConfigFS).

**Workaround: Android Open Accessory (AOA) 2.0**
Rignite (Host) must send AOA control packets to switch the Phone into "Accessory Mode".
1.  **Rignite** sends `51` (Get Protocol).
2.  **Rignite** sends string IDs (Manufacturer: "Rignite", Model: "RDF", etc.).
3.  **Rignite** sends `53` (Start).
4.  **Phone** re-enumerates with VID `0x18D1` and PID `0x2D00` (or similar).
5.  **Android App** launches automatically via `usb-accessory` filter.
6.  **Android App** gets a `ParcelFileDescriptor` to read/write raw bytes.

**Android App Code (AOA):**

```java
UsbManager manager = (UsbManager) getSystemService(Context.USB_SERVICE);
UsbAccessory[] accessories = manager.getAccessoryList();
UsbAccessory accessory = (accessories == null ? null : accessories[0]);

if (accessory != null) {
    ParcelFileDescriptor pfd = manager.openAccessory(accessory);
    FileDescriptor fd = pfd.getFileDescriptor();
    FileInputStream inputStream = new FileInputStream(fd);
    FileOutputStream outputStream = new FileOutputStream(fd);

    // 1. Wait for "READY" command from Rignite
    // 2. Send RDF Header
    // 3. Stream Data
}
```

**Manifest for AOA:**

```xml
<activity ...>
    <intent-filter>
        <action android:name="android.hardware.usb.action.USB_ACCESSORY_ATTACHED" />
    </intent-filter>
    <meta-data android:name="android.hardware.usb.action.USB_ACCESSORY_ATTACHED"
               android:resource="@xml/accessory_filter" />
</activity>
```

**res/xml/accessory_filter.xml**
```xml
<resources>
    <usb-accessory model="RDF" manufacturer="Rignite" version="1.0" />
</resources>
```

## 5. Future Roadmap

1.  **Prototype Phase:** Use `QEMU` + `socat` to emulate the "Pipe" over a TCP socket or local file, verifying the Rignite UEFI logic processes the RDF Header correctly.
2.  **Alpha Phase:** Implement RNDIS (USB Tethering) support in Rignite. This is safer than writing a raw USB Host driver in Rust for AOA.
3.  **Beta Phase:** Implement full AOA protocol in Rignite to allow "Plug and Pray" recovery without user enabling tethering settings on the phone.

## 6. Build and Test Instructions

### Building the Android App
Source code has been generated in `Rignite/android_app`.

1.  Navigate to the directory:
    ```bash
    cd Rignite/android_app
    ```
2.  Run the build script (downloads JDK 17 & Gradle automatically):
    ```bash
    ./build_app.sh
    ```
3.  The APK will be located at:
    `app/build/outputs/apk/debug/app-debug.apk`

### Testing with Real Hardware (UEFI)
1.  Install the APK on an Android device.
2.  Connect the device to the PC running Rignite (UEFI).
3.  Launch Rignite and select "Recovery".
4.  If the device is not detected as an Accessory (VID 0x18D1), Rignite will attempt the AOA handshake.
5.  Accept the USB permission prompt on Android.
6.  Select a file (OS Image) in the App and tap "Flash".

### Testing with QEMU (Advanced)
USB Passthrough of a mobile device to QEMU is complex because the VID/PID changes during the AOA handshake.
1.  Start QEMU with USB Host support.
2.  Pass the phone's initial VID/PID.
3.  When Rignite performs the handshake, the phone disconnects.
4.  You must quickly pass the NEW VID/PID (0x18D1:0x2D00/01) to the QEMU monitor.