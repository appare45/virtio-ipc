#![no_std]

#[cfg(feature = "debug-log")]
macro_rules! vq_debug {
    ($($arg:tt)*) => { ::log::debug!($($arg)*) };
}

#[cfg(not(feature = "debug-log"))]
macro_rules! vq_debug {
    ($($arg:tt)*) => {};
}

#[allow(unused_imports)]
pub(crate) use vq_debug;

pub const VIRTQ_DESC_F_WRITE: u16 = 1 << 1;
pub const VIRTQ_DESC_F_AVAIL: u16 = 1 << 7;
pub const VIRTQ_DESC_F_USED: u16 = 1 << 15;

/// §2.8.13 pvirtq_desc
#[repr(C)]
#[derive(Clone, Copy)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub id: u16,
    pub flags: u16,
}

/// §2.8.14 pvirtq_event_suppress
#[repr(C)]
#[derive(Clone, Copy)]
pub struct EventSuppress {
    pub desc: u16,
    pub flags: u16,
}

/// §2.8 Packed Virtqueue
///
/// - Descriptor Area: desc_ring (アライメント 16)
/// - Device Area:     device_event_suppress (アライメント 4)
/// - Driver Area:     driver_event_suppress (アライメント 4)
#[repr(C)]
pub struct Virtqueue<const QUEUE_SIZE: usize> {
    pub desc_ring: [VirtqDesc; QUEUE_SIZE],
    pub device_event_suppress: EventSuppress,
    pub driver_event_suppress: EventSuppress,
}

pub mod device;
pub mod driver;
