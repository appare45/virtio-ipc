use std::sync::atomic::{fence, Ordering};

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

/// ドライバ側。descriptor ring へのバッファ供給と回収を担う。
///
/// バッファ id（= スロット番号 0..num）で各バッファを識別する。
/// 呼び出し側は alloc_id でスロットを確保してバッファを準備し、
/// place_buffer で available として公開する。
/// get_used_id で used 済み id が返ったらバッファを回収・再利用する。
///
/// place_buffer / get_used_id はドライバスレッドのみが呼ぶ。
pub struct DriverVirtq {
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
    /// 空き id のスタック（id = バッファスロット番号 0..num）
    free_ids: Vec<u16>,
}

unsafe impl Send for DriverVirtq {}

impl DriverVirtq {
    pub fn new(desc: *mut VirtqDesc, num: usize) -> Self {
        let free_ids = (0..num as u16).rev().collect();
        DriverVirtq {
            num,
            desc,
            avail_idx: 0,
            driver_wrap: true,
            used_idx: 0,
            device_wrap: true,
            free_ids,
        }
    }

    /// 空き id を1つ確保して返す。満杯なら None。
    /// 返値の id に対応するバッファスロットを準備してから place_buffer を呼ぶ。
    pub fn alloc_id(&mut self) -> Option<u16> {
        self.free_ids.pop()
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
        fence(Ordering::Acquire);
        let flags = d.flags;
        let used = (flags & VIRTQ_DESC_F_USED) != 0;
        let avail = (flags & VIRTQ_DESC_F_AVAIL) != 0;
        // §2.8.1: used は AVAIL == USED == device_wrap
        if used != self.device_wrap || avail != self.device_wrap {
            return None;
        }
        let id = d.id;
        self.free_ids.push(id);

        self.used_idx = self.used_idx.wrapping_add(1);
        if self.used_idx as usize % self.num == 0 {
            self.device_wrap = !self.device_wrap;
        }
        Some(id)
    }
}

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
        DeviceVirtq { num, desc, next: 0, wrap: true }
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
