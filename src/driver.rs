use core::sync::atomic::{Ordering, fence};

use crate::{VIRTQ_DESC_F_AVAIL, VIRTQ_DESC_F_USED, VIRTQ_DESC_F_WRITE, Virtqueue, VirtqDesc};

const FREE_LIST_END: u16 = u16::MAX;

/// ドライバ側。descriptor ring へのバッファ供給と回収を担う。
///
/// バッファ id（= スロット番号 0..num）で各バッファを識別する。
/// 呼び出し側は alloc_id でスロットを確保してバッファを準備し、
/// place_buffer で available として公開する。
/// get_used_id で used 済み id が返ったらバッファを回収・再利用する。
///
/// place_buffer / get_used_id はドライバスレッドのみが呼ぶ。
pub struct DriverVirtq<'a> {
    num: usize,
    desc: *mut VirtqDesc,
    /// 次に available を書くリング位置（単調増加、% num でインデックス化）
    avail_idx: u16,
    /// Driver Ring Wrap Counter（§2.8.1、初期値 1）
    driver_wrap: bool,
    /// 次に used を確認するリング位置（単調増加）
    used_idx: u16,
    /// Device Ring Wrap Counter のドライバ側ミラー（§2.8.1）
    device_wrap: bool,
    free_next: &'a mut [u16],
    free_head: u16,
}

unsafe impl Send for DriverVirtq<'_> {}

impl<'a> DriverVirtq<'a> {
    pub fn new<const N: usize>(vq: *mut Virtqueue<N>, free_next: &'a mut [u16]) -> Self {
        let num = N;
        let desc = unsafe { (*vq).desc_ring.as_mut_ptr() };
        Self::from_raw(desc, num, free_next)
    }

    fn from_raw(desc: *mut VirtqDesc, num: usize, free_next: &'a mut [u16]) -> Self {
        assert_eq!(free_next.len(), num);
        for (id, next) in free_next.iter_mut().enumerate() {
            *next = if id + 1 < num {
                id as u16 + 1
            } else {
                FREE_LIST_END
            };
        }
        DriverVirtq {
            num,
            desc,
            avail_idx: 0,
            driver_wrap: true,
            used_idx: 0,
            device_wrap: true,
            free_next,
            free_head: 0,
        }
    }

    /// 空き id を1つ確保して返す。満杯なら None。
    /// 返値の id に対応するバッファスロットを準備してから place_buffer を呼ぶ。
    pub fn alloc_id(&mut self) -> Option<u16> {
        if self.free_head == FREE_LIST_END {
            return None;
        }
        let id = self.free_head;
        self.free_head = self.free_next[id as usize];
        Some(id)
    }

    fn desc_at(&self, linear_idx: u16) -> &mut VirtqDesc {
        let slot = linear_idx as usize % self.num;
        unsafe { &mut *self.desc.add(slot) }
    }

    /// alloc_id で確保した id のバッファを available として公開する。
    /// addr: バッファの物理/仮想アドレス、len: バイト数、writable: デバイスが書き込む場合 true。
    pub fn place_buffer(&mut self, id: u16, addr: u64, len: u32, writable: bool) {
        let d = self.desc_at(self.avail_idx);
        d.addr = addr;
        d.len = len;
        d.id = id;

        // §2.8.1: available は AVAIL=driver_wrap, USED=逆（AVAIL ≠ USED）
        let mut flags = if writable { VIRTQ_DESC_F_WRITE } else { 0 };
        if self.driver_wrap {
            flags |= VIRTQ_DESC_F_AVAIL;
        } else {
            flags |= VIRTQ_DESC_F_USED;
        }
        fence(Ordering::Release);
        d.flags = flags;

        self.avail_idx = self.avail_idx.wrapping_add(1);
        if self.avail_idx as usize % self.num == 0 {
            self.driver_wrap = !self.driver_wrap;
        }
    }

    /// デバイスが used 化したバッファの id を1つ回収する。
    /// used descriptor がなければ None（ノンブロッキング）。
    /// 返値の id に対応するバッファスロットを再利用できる。
    pub fn get_used_id(&mut self) -> Option<u16> {
        let d = self.desc_at(self.used_idx);
        let flags = d.flags;
        fence(Ordering::Acquire);
        let used = (flags & VIRTQ_DESC_F_USED) != 0;
        let avail = (flags & VIRTQ_DESC_F_AVAIL) != 0;
        // §2.8.1: used は AVAIL == USED == device_wrap
        if used != self.device_wrap || avail != self.device_wrap {
            return None;
        }
        let id = d.id;
        self.free_next[id as usize] = self.free_head;
        self.free_head = id;

        self.used_idx = self.used_idx.wrapping_add(1);
        if self.used_idx as usize % self.num == 0 {
            self.device_wrap = !self.device_wrap;
        }
        Some(id)
    }
}
