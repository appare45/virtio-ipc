use std::thread;
use virtio_ipc::{BufferElement, DeviceVirtq, DriverVirtq, VirtqDesc};

const QUEUE_SIZE: usize = 64;
const MSG_COUNT: usize = 1_000_000;

#[repr(C)]
struct Message {
    id: u32,
    value: f64,
}

fn main() {
    let mut desc_ring: Vec<VirtqDesc> = (0..QUEUE_SIZE)
        .map(|_| VirtqDesc { addr: 0, len: 0, id: 0, flags: 0 })
        .collect();

    let ptr = desc_ring.as_mut_ptr() as usize;

    // デバイススレッド: available descriptorをポーリングして処理・完了通知。
    // バッファのメモリは読むだけ。解放はドライバ側の責務。
    let mut device_vq = DeviceVirtq::new(ptr as *mut VirtqDesc, QUEUE_SIZE);
    let device = thread::spawn(move || {
        let mut received = 0usize;
        while received < MSG_COUNT {
            let Some((addr, _len)) = device_vq.device_take_available() else {
                std::hint::spin_loop();
                continue;
            };
            let msg = unsafe { &*(addr as *const Message) };
            if received % 100_000 == 0 {
                println!("[device] recv #{}: id={} value={:.1}", received, msg.id, msg.value);
            }
            device_vq.device_complete();
            received += 1;
        }
    });

    // ドライバ（main）: 送信と回収を同じスレッド・同じ DriverVirtq で管理。
    // リング満杯のときは先に get_buffer で回収してスペースを空けてから送信する。
    let mut driver_vq = DriverVirtq::new(ptr as *mut VirtqDesc, QUEUE_SIZE);
    let mut reclaimed = 0usize;

    for sent in 0..MSG_COUNT {
        // リング満杯なら回収してスペースを空ける（少なくとも1件回収するまで待つ）
        while driver_vq.is_full() {
            if let Some(buf) = driver_vq.get_buffer() {
                let _ = unsafe { Box::from_raw(buf.addr as *mut Message) };
                reclaimed += 1;
            } else {
                std::hint::spin_loop();
            }
        }

        let msg = Box::new(Message { id: sent as u32, value: sent as f64 * 0.1 });
        let raw = Box::into_raw(msg);
        if sent % 100_000 == 0 {
            println!("[driver] send #{}: id={} value={:.1}", sent, unsafe { (*raw).id }, unsafe { (*raw).value });
        }
        // is_full チェック後なので必ず成功する
        driver_vq.place_buffer(BufferElement {
            addr: raw as *const u64,
            len: std::mem::size_of::<Message>() as u32,
            writable: false,
        });
    }

    // 回収フェーズ: 残り全件をデバイスがcompleteするまで待って回収
    while reclaimed < MSG_COUNT {
        if let Some(buf) = driver_vq.get_buffer() {
            let _ = unsafe { Box::from_raw(buf.addr as *mut Message) };
            reclaimed += 1;
        } else {
            std::hint::spin_loop();
        }
    }

    device.join().unwrap();
    println!("done. reclaimed {}/{} buffers", reclaimed, MSG_COUNT);
}
