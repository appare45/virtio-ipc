use std::mem::size_of;

const VIRTQ_LEN: usize = 1;
const DEVICE_NUMBER: u32 = 65535;
const QUEUE_SIZE: usize = 1024;

const VIRTQ_DESC_F_NEXT: u16 = 1;
const VIRTQ_DESC_F_WRITE: u16 = 2;

#[repr(C)]
struct VirtqDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}
const _: () = assert!(size_of::<VirtqDesc>() == 16);

#[repr(C)]
struct VirtqAvail {
    flags: u16,
    idx: u16,
    ring: [u16; QUEUE_SIZE],
}

#[repr(C)]
struct VirtqUsedElem {
    id: u32,
    len: u32,
}

#[repr(C)]
struct VirtqUsed {
    flags: u16,
    idx: u16,
    ring: [VirtqUsedElem; QUEUE_SIZE],
}

struct Virtq {
    num: usize,
    desc: *mut VirtqDesc,
    avail: *mut VirtqAvail,
    used: *mut VirtqUsed,
    free_head: u16,
    free_count: u16,
}

struct BufferElement {
    addr: u64,
    len: u32,
    writable: bool,
}

fn virtq_need_event(event_idx: u16, new_idx: u16, old_idx: u16) -> bool {
    (new_idx.wrapping_sub(event_idx).wrapping_sub(1)) < new_idx.wrapping_sub(old_idx)
}

unsafe fn virtq_used_event(vq: *const Virtq) -> *mut u16 {
    // used event index は avail ring の末尾にある（後方互換性のため）
    unsafe { (*(*vq).avail).ring.as_ptr().add((*vq).num) as *mut u16 }
}

unsafe fn virtq_avail_event(vq: *const Virtq) -> *mut u16 {
    // avail event index は used ring の末尾にある（後方互換性のため）
    unsafe { (*(*vq).used).ring.as_ptr().add((*vq).num) as *mut u16 }
}

fn main() {
    println!("Hello, world!");
}
