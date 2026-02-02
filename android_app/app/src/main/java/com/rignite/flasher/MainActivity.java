package com.rignite.flasher;

import android.app.Activity;
import android.app.PendingIntent;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.database.Cursor;
import android.hardware.usb.UsbAccessory;
import android.hardware.usb.UsbManager;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.provider.OpenableColumns;
import android.util.Log;
import android.widget.Button;
import android.widget.TextView;
import android.widget.Toast;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;
import androidx.appcompat.app.AppCompatActivity;
import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.ByteBuffer;
import java.nio.ByteOrder;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.security.NoSuchAlgorithmException;
import java.util.Arrays;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public class MainActivity extends AppCompatActivity {

  private static final String TAG = "RigniteFlasher";
  private static final String ACTION_USB_PERMISSION =
    "com.rignite.flasher.USB_PERMISSION";

  private TextView tvStatus;
  private TextView tvSelectedFile;
  private TextView tvLog;
  private Button btnSelectFile;
  private Button btnFlash;

  private UsbManager usbManager;
  private UsbAccessory currentAccessory;
  private ParcelFileDescriptor fileDescriptor;
  private FileOutputStream outputStream;

  private Uri selectedFileUri;
  private long selectedFileSize;
  private String selectedFileName;

  private final ExecutorService executor = Executors.newSingleThreadExecutor();

  // BroadcastReceiver for USB permission
  private final BroadcastReceiver usbReceiver = new BroadcastReceiver() {
    @Override
    public void onReceive(Context context, Intent intent) {
      String action = intent.getAction();
      if (ACTION_USB_PERMISSION.equals(action)) {
        synchronized (this) {
          UsbAccessory accessory = intent.getParcelableExtra(
            UsbManager.EXTRA_ACCESSORY
          );
          if (
            intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
          ) {
            if (accessory != null) {
              openAccessory(accessory);
            }
          } else {
            log("USB permission denied for accessory " + accessory);
          }
        }
      } else if (UsbManager.ACTION_USB_ACCESSORY_DETACHED.equals(action)) {
        UsbAccessory accessory = intent.getParcelableExtra(
          UsbManager.EXTRA_ACCESSORY
        );
        if (accessory != null && accessory.equals(currentAccessory)) {
          closeAccessory();
        }
      }
    }
  };

  private final ActivityResultLauncher<Intent> filePickerLauncher =
    registerForActivityResult(
      new ActivityResultContracts.StartActivityForResult(),
      result -> {
        if (
          result.getResultCode() == Activity.RESULT_OK &&
          result.getData() != null
        ) {
          selectedFileUri = result.getData().getData();
          queryFileMetadata(selectedFileUri);
          btnFlash.setEnabled(
            currentAccessory != null && selectedFileUri != null
          );
        }
      }
    );

  @Override
  protected void onCreate(Bundle savedInstanceState) {
    super.onCreate(savedInstanceState);
    setContentView(R.layout.activity_main);

    tvStatus = findViewById(R.id.tvStatus);
    tvSelectedFile = findViewById(R.id.tvSelectedFile);
    tvLog = findViewById(R.id.tvLog);
    btnSelectFile = findViewById(R.id.btnSelectFile);
    btnFlash = findViewById(R.id.btnFlash);

    usbManager = (UsbManager) getSystemService(Context.USB_SERVICE);

    // register receiver for USB permission
    IntentFilter filter = new IntentFilter(ACTION_USB_PERMISSION);
    filter.addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED);

    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
      registerReceiver(usbReceiver, filter, Context.RECEIVER_NOT_EXPORTED);
    } else {
      registerReceiver(usbReceiver, filter);
    }

    btnSelectFile.setOnClickListener(v -> openFilePicker());
    btnFlash.setOnClickListener(v -> startFlashing());

    handleIntent(getIntent());
  }

  @Override
  protected void onResume() {
    super.onResume();

    if (currentAccessory != null) {
      return;
    }

    if (usbManager != null) {
      UsbAccessory[] accessories = usbManager.getAccessoryList();
      if (accessories != null) {
        for (UsbAccessory accessory : accessories) {
          if (usbManager.hasPermission(accessory)) {
            openAccessory(accessory);
          } else {
            PendingIntent permissionIntent = PendingIntent.getBroadcast(
              this,
              0,
              new Intent(ACTION_USB_PERMISSION),
              PendingIntent.FLAG_IMMUTABLE
            );
            usbManager.requestPermission(accessory, permissionIntent);
          }
          // We only support connecting to one accessory at a time
          break;
        }
      }
    }
  }

  @Override
  protected void onNewIntent(Intent intent) {
    super.onNewIntent(intent);
    handleIntent(intent);
  }

  @Override
  protected void onDestroy() {
    super.onDestroy();
    unregisterReceiver(usbReceiver);
    closeAccessory();
  }

  private void handleIntent(Intent intent) {
    if (UsbManager.ACTION_USB_ACCESSORY_ATTACHED.equals(intent.getAction())) {
      UsbAccessory accessory = intent.getParcelableExtra(
        UsbManager.EXTRA_ACCESSORY
      );
      if (accessory != null) {
        openAccessory(accessory);
      }
    }
  }

  private void openAccessory(UsbAccessory accessory) {
    fileDescriptor = usbManager.openAccessory(accessory);
    if (fileDescriptor != null) {
      currentAccessory = accessory;
      FileDescriptor fd = fileDescriptor.getFileDescriptor();
      outputStream = new FileOutputStream(fd);

      tvStatus.setText(R.string.status_connected);
      log("Connected to: " + accessory.getDescription());

      if (selectedFileUri != null) {
        btnFlash.setEnabled(true);
      }
    } else {
      log("Failed to open accessory.");
    }
  }

  private void closeAccessory() {
    try {
      if (fileDescriptor != null) {
        fileDescriptor.close();
      }
    } catch (IOException e) {
      // ignore
    } finally {
      fileDescriptor = null;
      currentAccessory = null;
      outputStream = null;
      runOnUiThread(() -> {
        tvStatus.setText(R.string.status_disconnected);
        btnFlash.setEnabled(false);
        log("Disconnected.");
      });
    }
  }

  private void openFilePicker() {
    Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT);
    intent.addCategory(Intent.CATEGORY_OPENABLE);
    intent.setType("*/*");
    filePickerLauncher.launch(intent);
  }

  private void queryFileMetadata(Uri uri) {
    try (
      Cursor cursor = getContentResolver().query(uri, null, null, null, null)
    ) {
      if (cursor != null && cursor.moveToFirst()) {
        int nameIndex = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME);
        int sizeIndex = cursor.getColumnIndex(OpenableColumns.SIZE);

        if (nameIndex != -1) selectedFileName = cursor.getString(nameIndex);
        if (sizeIndex != -1) selectedFileSize = cursor.getLong(sizeIndex);

        tvSelectedFile.setText(
          "Selected: " + selectedFileName + " (" + selectedFileSize + " bytes)"
        );
        log("File selected: " + selectedFileName);
      }
    } catch (Exception e) {
      log("Error getting file info: " + e.getMessage());
    }
  }

  private void startFlashing() {
    if (outputStream == null || selectedFileUri == null) return;

    btnFlash.setEnabled(false);
    btnSelectFile.setEnabled(false);
    log("Starting flash process...");

    executor.execute(() -> {
      try {
        // 1. Calculate Checksum
        log("Calculating checksum...");
        byte[] checksum = calculateChecksum(selectedFileUri);

        // 2. Prepare Header
        byte[] header = createRdfHeader(selectedFileSize, checksum);

        // 3. Send Header
        log("Sending Header...");
        outputStream.write(header);

        // 3. Stream File
        try (
          InputStream inputStream = getContentResolver().openInputStream(
            selectedFileUri
          )
        ) {
          if (inputStream == null) throw new IOException(
            "Cannot open file stream"
          );

          byte[] buffer = new byte[64 * 1024]; // 64KB chunks
          long totalSent = 0;
          int bytesRead;
          int lastProgress = -1;

          log("Streaming data...");
          while ((bytesRead = inputStream.read(buffer)) != -1) {
            outputStream.write(buffer, 0, bytesRead);
            totalSent += bytesRead;

            int progress = (int) ((totalSent * 100) / selectedFileSize);
            if (progress > lastProgress) {
              final long sent = totalSent;
              runOnUiThread(() ->
                tvLog.setText("Sending: " + progress + "% (" + sent + " bytes)")
              );
              lastProgress = progress;
            }
          }
          outputStream.flush();
        }

        runOnUiThread(() -> {
          log("Flash Complete!");
          Toast.makeText(this, "Flash Complete", Toast.LENGTH_SHORT).show();
        });
      } catch (Exception e) {
        runOnUiThread(() -> log("Error during flashing: " + e.getMessage()));
        Log.e(TAG, "Flash error", e);
      } finally {
        runOnUiThread(() -> {
          btnFlash.setEnabled(true);
          btnSelectFile.setEnabled(true);
        });
      }
    });
  }

  private byte[] calculateChecksum(Uri uri)
    throws IOException, NoSuchAlgorithmException {
    try (InputStream inputStream = getContentResolver().openInputStream(uri)) {
      if (inputStream == null) throw new IOException(
        "Cannot open file for checksum"
      );

      MessageDigest digest = MessageDigest.getInstance("SHA-256");
      byte[] buffer = new byte[64 * 1024];
      int bytesRead;

      while ((bytesRead = inputStream.read(buffer)) != -1) {
        digest.update(buffer, 0, bytesRead);
      }
      return digest.digest();
    }
  }

  private byte[] createRdfHeader(long imageSize, byte[] checksum) {
    ByteBuffer buffer = ByteBuffer.allocate(128);
    buffer.order(ByteOrder.LITTLE_ENDIAN);

    // Magic: 0x52 0x44 0x46 0x21 ("RDF!")
    buffer.put(new byte[] { 0x52, 0x44, 0x46, 0x21 });

    // Image Size (u64)
    buffer.putLong(imageSize);

    // Checksum (32 bytes)
    if (checksum.length != 32) {
      throw new IllegalArgumentException("Checksum must be 32 bytes");
    }
    buffer.put(checksum);

    // Target Subvolume (64 bytes, zero padded)
    byte[] target = "@core".getBytes(StandardCharsets.UTF_8);
    byte[] targetField = new byte[64];
    System.arraycopy(target, 0, targetField, 0, Math.min(target.length, 64));
    buffer.put(targetField);

    // Reserved (20 bytes)
    buffer.put(new byte[20]);

    return buffer.array();
  }

  private void log(String message) {
    runOnUiThread(() -> {
      String current = tvLog.getText().toString();
      // keep log short
      if (current.length() > 1000) current = current.substring(0, 1000);
      tvLog.setText(message + "\n" + current);
    });
  }
}
