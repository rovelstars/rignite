use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

pub struct LogEntry {
    pub level: &'static str,
    pub message: String,
}

static mut LOG_BUFFER: Option<Vec<LogEntry>> = None;

/// Initialize the in-memory logger
pub fn init() {
    unsafe {
        if LOG_BUFFER.is_none() {
            LOG_BUFFER = Some(Vec::with_capacity(50));
        }
    }
}

/// Internal logging function called by macros
pub fn log(level: &'static str, args: fmt::Arguments) {
    // 1. Attempt to print to standard UEFI output (Serial/ConOut)
    uefi::println!("[{}] {}", level, args);

    // 2. Store in memory for on-screen debug console
    unsafe {
        if let Some(buffer) = &mut LOG_BUFFER {
            // Circular-ish buffer: Remove oldest if full
            if buffer.len() >= 50 {
                buffer.remove(0);
            }

            // We use alloc::format! to convert arguments to String
            // This requires the global allocator to be set up
            let message = alloc::format!("{}", args);

            buffer.push(LogEntry { level, message });
        }
    }
}

/// Retrieve current log entries (unsafe access, purely for single-threaded rendering)
pub fn get_logs() -> &'static [LogEntry] {
    unsafe {
        match &LOG_BUFFER {
            Some(buffer) => buffer.as_slice(),
            None => &[],
        }
    }
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {
        #[cfg(debug_assertions)]
        $crate::logger::log("DBG", format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! info {
    ($($arg:tt)*) => {
        $crate::logger::log("INF", format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! warn {
    ($($arg:tt)*) => {
        $crate::logger::log("WRN", format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! error {
    ($($arg:tt)*) => {
        $crate::logger::log("ERR", format_args!($($arg)*));
    };
}
