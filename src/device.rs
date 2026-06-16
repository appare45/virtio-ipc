use std::sync::atomic::{Ordering, fence};

use crate::{VIRTQ_DESC_F_AVAIL, VIRTQ_DESC_F_USED, VirtqDesc};

/// デバイス側。descriptor ring から available バッファを取り出し、完了を通知する。
/// device_take_available / device_complete はデバイススレッドのみが呼ぶ。
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
    pub fn new(desc: *mut VirtqDesc, num: usize) -> Self {
        DeviceVirtq {
            num,
            desc,
            next: 0,
            wrap: true,
        }
    }

    fn desc_at(&self, idx: u16) -> &mut VirtqDesc {
        unsafe { &mut *self.desc.add(idx as usize) }
    }

    /// 次の available descriptor を取得する。なければ None（ノンブロッキング）。
    /// 返値: (addr, len, id)
    pub fn device_take_available(&self) -> Option<(u64, u32, u16)> {
        let d = self.desc_at(self.next);
        fence(Ordering::Acquire);
        let flags = d.flags;
        let avail = (flags & VIRTQ_DESC_F_AVAIL) != 0;
        let used = (flags & VIRTQ_DESC_F_USED) != 0;
        // §2.8.1: available は AVAIL=device_wrap かつ AVAIL ≠ USED
        if avail != self.wrap || avail == used {
            return None;
        }
        Some((d.addr, d.len, d.id))
    }

    /// 現在の next スロットを used 完了にしてカーソルを進める。
    /// device_take_available で取得済みのスロットに対して呼ぶ。
    pub fn device_complete(&mut self) {
        let d = self.desc_at(self.next);
        // §2.8.1: used は AVAIL == USED == device_wrap
        let mut flags = d.flags & !(VIRTQ_DESC_F_AVAIL | VIRTQ_DESC_F_USED);
        if self.wrap {
            flags |= VIRTQ_DESC_F_AVAIL | VIRTQ_DESC_F_USED;
        }
        fence(Ordering::Release);
        d.flags = flags;

        self.next += 1;
        if self.next as usize >= self.num {
            self.next = 0;
            self.wrap = !self.wrap;
        }
    }
}
