use std::thread;
use virtio_ipc::{DeviceVirtq, DriverVirtq, VirtqDesc};

const QUEUE_SIZE: usize = 64;
const MSG_COUNT: usize = 1_000_000;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Message {
    seq: u32,
    value: f64,
}

fn main() {
    // descriptor ring を一括確保
    let mut desc_ring: Vec<VirtqDesc> =
        (0..QUEUE_SIZE).map(|_| VirtqDesc { addr: 0, len: 0, id: 0, flags: 0 }).collect();

    // バッファプールを一括確保（id = インデックス、使い回す）
    let mut pool: Vec<Message> = vec![Message::default(); QUEUE_SIZE];

    let desc_ptr = desc_ring.as_mut_ptr() as usize;
    let pool_ptr = pool.as_mut_ptr() as usize;
    let msg_size = std::mem::size_of::<Message>() as u32;

    // デバイススレッド: available を取り出して読み、complete する。
    let mut device_vq = DeviceVirtq::new(desc_ptr as *mut VirtqDesc, QUEUE_SIZE);
    let device = thread::spawn(move || {
        let mut received = 0usize;
        while received < MSG_COUNT {
            let Some((addr, _len, _id)) = device_vq.device_take_available() else {
                std::hint::spin_loop();
                continue;
            };
            let msg = unsafe { &*(addr as *const Message) };
            if received % 100_000 == 0 {
                println!("[device] recv #{}: seq={} value={:.1}", received, msg.seq, msg.value);
            }
            device_vq.device_complete();
            received += 1;
        }
    });

    // ドライバ（main スレッド）: alloc_id → pool[id] に書く → place_buffer。
    // get_used_id で回収した id は次の送信に再利用する。
    let mut driver_vq = DriverVirtq::new(desc_ptr as *mut VirtqDesc, QUEUE_SIZE);
    let pool = unsafe { std::slice::from_raw_parts_mut(pool_ptr as *mut Message, QUEUE_SIZE) };

    let mut sent = 0usize;
    let mut reclaimed = 0usize;

    while sent < MSG_COUNT || reclaimed < MSG_COUNT {
        // 回収: used 済みがあれば先に処理（id をプールに戻す）
        while let Some(_id) = driver_vq.get_used_id() {
            reclaimed += 1;
        }

        // 送信: 空き id があり、まだ送るものがあれば place
        while sent < MSG_COUNT {
            let Some(id) = driver_vq.alloc_id() else { break };
            // pool[id] に書いてから place する（デバイスに見える前に書く）
            pool[id as usize] = Message { seq: sent as u32, value: sent as f64 * 0.1 };
            let addr = &pool[id as usize] as *const Message as u64;
            if sent % 100_000 == 0 {
                println!("[driver] send #{}: seq={} value={:.1}", sent, pool[id as usize].seq, pool[id as usize].value);
            }
            driver_vq.place_buffer(id, addr, msg_size, false);
            sent += 1;
        }

        std::hint::spin_loop();
    }

    device.join().unwrap();
    println!("done. sent={} reclaimed={}", sent, reclaimed);
}
