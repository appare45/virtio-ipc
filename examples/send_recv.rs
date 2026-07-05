use std::thread;
use std::time::Duration;
use rand::Rng;
use virtio_ipc::{EventSuppress, Virtqueue, VirtqDesc, device::DeviceVirtq, driver::DriverVirtq};

const QUEUE_SIZE: usize = 64;
const MSG_COUNT: usize = 1_000;

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Message {
    seq: u32,
    value: f64,
}

fn run_device(mut device_vq: DeviceVirtq) {
    let mut rng = rand::thread_rng();
    let mut received = 0usize;
    let mut starved = false;
    while received < MSG_COUNT {
        let Some((addr, _len, _id)) = device_vq.device_take_available() else {
            if !starved {
                println!("[device] no buffer available! waiting for driver (received={})", received);
                starved = true;
            }
            std::hint::spin_loop();
            continue;
        };
        starved = false;
        let msg = unsafe { &*(addr as *const Message) };
        println!(
            "[device] recv #{}: seq={} value={:.1}",
            received, msg.seq, msg.value
        );
        // ランダム遅延: 0〜15ms (時々速くなり、driverがバッファ待ちになる)
        let delay_ms: u64 = rng.gen_range(0..15);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        device_vq.device_complete();
        received += 1;
    }
}

fn run_driver(mut driver_vq: DriverVirtq, pool: &mut [Message]) {
    let msg_size = std::mem::size_of::<Message>() as u32;
    let mut sent = 0usize;
    let mut reclaimed = 0usize;

    let mut rng = rand::thread_rng();
    while sent < MSG_COUNT || reclaimed < MSG_COUNT {
        while driver_vq.get_used_id().is_some() {
            reclaimed += 1;
        }

        while sent < MSG_COUNT {
            let Some(id) = driver_vq.alloc_id() else {
                println!("[driver] queue full! waiting for device to catch up (sent={} reclaimed={})", sent, reclaimed);
                break;
            };
            pool[id as usize] = Message {
                seq: sent as u32,
                value: sent as f64 * 0.1,
            };
            let addr = &pool[id as usize] as *const Message as u64;
            println!(
                "[driver] send #{}: seq={} value={:.1}",
                sent, pool[id as usize].seq, pool[id as usize].value
            );
            // ランダム遅延: 0〜20ms (時々遅くなり、deviceがバッファ待ちになる)
            let delay_ms: u64 = rng.gen_range(0..20);
            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
            driver_vq.place_buffer(id, addr, msg_size, false);
            sent += 1;
        }

        std::hint::spin_loop();
    }

    println!("done. sent={} reclaimed={}", sent, reclaimed);
}

fn main() {
    let suppress = EventSuppress { desc: 0, flags: 0 };
    let mut vq: Box<Virtqueue<QUEUE_SIZE>> = Box::new(Virtqueue {
        desc_ring: [VirtqDesc {
            addr: 0,
            len: 0,
            id: 0,
            flags: 0,
        }; QUEUE_SIZE],
        device_event_suppress: suppress,
        driver_event_suppress: suppress,
    });
    let mut pool: Vec<Message> = vec![Message::default(); QUEUE_SIZE];

    let vq_ptr = &mut *vq as *mut Virtqueue<QUEUE_SIZE> as usize;
    let pool_ptr = pool.as_mut_ptr() as usize;

    let device_vq = DeviceVirtq::new(vq_ptr as *mut Virtqueue<QUEUE_SIZE>);
    let device = thread::spawn(move || run_device(device_vq));

    let mut free_next = [0u16; QUEUE_SIZE];
    let driver_vq = DriverVirtq::new(vq_ptr as *mut Virtqueue<QUEUE_SIZE>, &mut free_next);
    let pool_slice =
        unsafe { std::slice::from_raw_parts_mut(pool_ptr as *mut Message, QUEUE_SIZE) };
    run_driver(driver_vq, pool_slice);

    device.join().unwrap();
}
