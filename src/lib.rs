#![no_std]

pub const VIRTQ_DESC_F_WRITE: u16 = 1 << 1;
pub const VIRTQ_DESC_F_AVAIL: u16 = 1 << 7;
pub const VIRTQ_DESC_F_USED: u16 = 1 << 15;

/// §2.8.13 pvirtq_desc
#[repr(C)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub id: u16,
    pub flags: u16,
}

unsafe impl Send for VirtqDesc {}

pub mod device;
pub mod driver;
