use core::sync::atomic::{Ordering, fence};

#[allow(unused_imports)]
use crate::{VIRTQ_DESC_F_AVAIL, VIRTQ_DESC_F_USED, Virtqueue, VirtqDesc, vq_debug};

pub struct DeviceVirtq {
    num: usize,
    desc: *mut VirtqDesc,
    /// 次に available を確認するリング位置
    next: u16,
    /// Device Ring Wrap Counter（§2.8.1、初期値 1）
    wrap: bool,
}

unsafe impl Send for DeviceVirtq {}

impl DeviceVirtq {
    pub fn new<const N: usize>(vq: *mut Virtqueue<N>) -> Self {
        DeviceVirtq {
            num: N,
            desc: unsafe { (*vq).desc_ring.as_mut_ptr() },
            next: 0,
            wrap: true,
        }
    }

    fn desc_at(&self, idx: u16) -> &mut VirtqDesc {
        unsafe { &mut *self.desc.add(idx as usize) }
    }

    pub fn device_take_available(&self) -> Option<(u64, u32, u16)> {
        let d = self.desc_at(self.next);
        let flags = d.flags;
        fence(Ordering::Acquire);
        let avail = (flags & VIRTQ_DESC_F_AVAIL) != 0;
        let used = (flags & VIRTQ_DESC_F_USED) != 0;
        vq_debug!(
            "[device] device_take_available: next={} wrap={} desc flags={:#06x} (avail={} used={})",
            self.next, self.wrap, flags, avail, used
        );
        // §2.8.1: available は AVAIL=device_wrap かつ AVAIL ≠ USED
        if avail != self.wrap || avail == used {
            vq_debug!("[device] device_take_available: no available descriptor");
            return None;
        }
        vq_debug!(
            "[device] device_take_available: found addr={:#x} len={} id={}",
            d.addr, d.len, d.id
        );
        Some((d.addr, d.len, d.id))
    }

    pub fn device_complete(&mut self) {
        let d = self.desc_at(self.next);
        // §2.8.1: AVAIL == USED == device_wrap_counter
        const WRAP_FLAGS: u16 = VIRTQ_DESC_F_AVAIL | VIRTQ_DESC_F_USED;
        // wrap済みの場合はAVAIL, USEDを反転させる
        let wrap_bits = if self.wrap { WRAP_FLAGS } else { 0 };
        let new_flags = (d.flags & !WRAP_FLAGS) | wrap_bits;
        vq_debug!(
            "[device] device_complete: next={} wrap={} desc flags {:#06x} -> {:#06x}",
            self.next, self.wrap, d.flags, new_flags
        );
        fence(Ordering::Release);
        d.flags = new_flags;

        self.next += 1;
        if self.next as usize >= self.num {
            self.next = 0;
            self.wrap = !self.wrap;
        }
        vq_debug!(
            "[device] device_complete done: next={} wrap={}",
            self.next, self.wrap
        );
    }
}
