use std::sync::atomic::{fence, Ordering};

pub const VIRTQ_DESC_F_WRITE: u16 = 1 << 1;
pub const VIRTQ_DESC_F_AVAIL: u16 = 1 << 7;
pub const VIRTQ_DESC_F_USED: u16 = 1 << 15;

#[repr(C)]
pub struct VirtqDesc {
    pub addr: u64,
    pub len: u32,
    pub id: u16,
    pub flags: u16,
}

unsafe impl Send for VirtqDesc {}

pub struct BufferElement {
    pub addr: *const u64,
    pub len: u32,
    pub writable: bool,
}

/// ドライバ側: descriptor ringへのバッファ供給と回収を担う。
/// place_buffer と get_buffer はドライバスレッドのみが呼ぶ。
pub struct DriverVirtq {
    num: usize,
    desc: *mut VirtqDesc,
    free_head: u16,
    driver_wrap: u16,   // Driver Ring Wrap Counter（初期値1）
    next_id: u16,
    used_head: u16,
    used_wrap: u16,     // ドライバが used_head の周回を追跡するカウンタ（初期値1）
}

unsafe impl Send for DriverVirtq {}

impl DriverVirtq {
    pub fn new(desc: *mut VirtqDesc, num: usize) -> Self {
        DriverVirtq {
            num,
            desc,
            free_head: 0,
            driver_wrap: 1,
            next_id: 0,
            used_head: 0,
            used_wrap: 1,
        }
    }

    fn desc(&self, idx: u16) -> &mut VirtqDesc {
        unsafe { &mut *self.desc.add(idx as usize) }
    }

    /// リングが満杯（送信済み未回収がnumに達した）かどうかを返す。
    pub fn is_full(&self) -> bool {
        self.free_head.wrapping_sub(self.used_head) as usize >= self.num
    }

    /// 仕様§2.8.21: バッファをdescriptor ringに供給する。
    /// リングが満杯のときは `false` を返す（ノンブロッキング）。
    /// 呼び出し側は満杯のとき get_buffer で回収してから再試行すること。
    pub fn place_buffer(&mut self, buf: BufferElement) -> bool {
        if self.is_full() {
            return false;
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let idx = (self.free_head as usize % self.num) as u16;
        let d = self.desc(idx);
        d.addr = buf.addr as u64;
        d.len = buf.len;
        d.id = id;

        // available descriptor は AVAIL != USED（片方のビットのみ）で表現する。
        // AVAIL を Driver Ring Wrap Counter に合わせ、USED はその逆にする。
        // これによりデバイスが used 化した状態（AVAIL == USED）と区別できる。
        let mut flags: u16 = 0;
        if buf.writable {
            flags |= VIRTQ_DESC_F_WRITE;
        }
        if self.driver_wrap == 1 {
            flags |= VIRTQ_DESC_F_AVAIL;
        } else {
            flags |= VIRTQ_DESC_F_USED;
        }

        fence(Ordering::Release);
        d.flags = flags;

        self.free_head = self.free_head.wrapping_add(1);
        // free_head が num の倍数を跨ぐ（= リングを1周する）たびに wrap counter をトグル。
        if self.free_head as usize % self.num == 0 {
            self.driver_wrap ^= 1;
        }
        true
    }

    /// 仕様§2.8.22: デバイスがused化したバッファを回収する。
    /// used化されていなければNoneを返す（ノンブロッキング）。
    pub fn get_buffer(&mut self) -> Option<BufferElement> {
        let idx = (self.used_head as usize % self.num) as u16;
        let d = self.desc(idx);
        fence(Ordering::Acquire);
        let flags = d.flags;
        let used = (flags & VIRTQ_DESC_F_USED) != 0;
        let avail = (flags & VIRTQ_DESC_F_AVAIL) != 0;
        let wrap = self.used_wrap == 1;
        // used descriptor は AVAIL == USED == used_wrap
        if used != wrap || avail != wrap {
            return None;
        }
        let (addr, len) = (d.addr, d.len);
        self.used_head = self.used_head.wrapping_add(1);
        if self.used_head as usize % self.num == 0 {
            self.used_wrap ^= 1;
        }
        Some(BufferElement {
            addr: addr as *const u64,
            len,
            writable: (flags & VIRTQ_DESC_F_WRITE) != 0,
        })
    }
}

/// デバイス側: descriptor ringからavailableバッファを取り出し、処理完了を通知する。
/// device_take_available と device_complete はデバイススレッドのみが呼ぶ。
pub struct DeviceVirtq {
    num: usize,
    desc: *mut VirtqDesc,
    next: u16,
    wrap: bool, // Device Ring Wrap Counter（初期値true=1）
}

unsafe impl Send for DeviceVirtq {}

impl DeviceVirtq {
    pub fn new(desc: *mut VirtqDesc, num: usize) -> Self {
        DeviceVirtq { num, desc, next: 0, wrap: true }
    }

    fn desc(&self, idx: u16) -> &mut VirtqDesc {
        unsafe { &mut *self.desc.add(idx as usize) }
    }

    /// 次のavailable descriptorを取り出す。available でなければ None（ノンブロッキング）。
    pub fn device_take_available(&self) -> Option<(*const u64, u32)> {
        let d = self.desc(self.next);
        fence(Ordering::Acquire);
        let flags = d.flags;
        let avail = (flags & VIRTQ_DESC_F_AVAIL) != 0;
        let used = (flags & VIRTQ_DESC_F_USED) != 0;
        // available descriptor は AVAIL == Device Ring Wrap Counter かつ AVAIL != USED。
        // used 化済み（AVAIL == USED）のスロットを誤検出しないよう両条件を確認する。
        if avail != self.wrap || avail == used {
            return None;
        }
        Some((d.addr as *const u64, d.len))
    }

    /// 現在の next スロットをused完了にしてカウンタを進める。
    /// device_take_available で取得済みのスロットに対して呼ぶ。
    pub fn device_complete(&mut self) {
        let d = self.desc(self.next);
        // 仕様§2.8.2: used descriptor は AVAIL == USED == Device Ring Wrap Counter。
        // AVAIL/USED 両ビットを device wrap counter に上書きする。
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
