/// プロセス間通信の例
///
/// 共有メモリ（POSIX shm_open + mmap）上に Virtqueue とメッセージプールを配置し、
/// fork した親子プロセス間で virtio packed virtqueue を使ってメッセージを送受信する。
///
/// レイアウト（共有メモリ内）:
///   [0 .. VQ_BYTES)   : Virtqueue<QUEUE_SIZE>
///   [VQ_BYTES .. ..)  : Message × QUEUE_SIZE  (メッセージプール)
use std::mem;
use std::num::NonZeroUsize;
use std::thread;
use std::time::Duration;
use rand::Rng;

use nix::fcntl::OFlag;
use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, ftruncate, fork};
use virtio_ipc::{Virtqueue, device::DeviceVirtq, driver::DriverVirtq};

const QUEUE_SIZE: usize = 64;
const MSG_COUNT: usize = 1_000;
const SHM_NAME: &str = "/virtio_ipc_example";

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Message {
    seq: u32,
    value: f64,
}

const VQ_BYTES: usize = mem::size_of::<Virtqueue<QUEUE_SIZE>>();
const POOL_BYTES: usize = mem::size_of::<Message>() * QUEUE_SIZE;
const SHM_SIZE: usize = VQ_BYTES + POOL_BYTES;

fn run_device(vq_ptr: *mut Virtqueue<QUEUE_SIZE>) {
    let mut device_vq = DeviceVirtq::new(vq_ptr);
    let pool_ptr = unsafe { (vq_ptr as *mut u8).add(VQ_BYTES) } as *const Message;

    let mut rng = rand::thread_rng();
    let mut starved = false;
    let mut received = 0usize;
    while received < MSG_COUNT {
        let Some((_addr, _len, id)) = device_vq.device_take_available() else {
            if !starved {
                println!("[device pid={}] no buffer available! waiting for driver (received={})", nix::unistd::getpid(), received);
                starved = true;
            }
            std::hint::spin_loop();
            continue;
        };
        starved = false;
        let msg = unsafe { &*pool_ptr.add(id as usize) };
        println!(
            "[device pid={}] recv #{}: seq={} value={:.1}",
            nix::unistd::getpid(),
            received,
            msg.seq,
            msg.value
        );
        // ランダム遅延: 0〜15ms (時々速くなり、driverがバッファ待ちになる)
        let delay_ms: u64 = rng.gen_range(0..15);
        if delay_ms > 0 {
            thread::sleep(Duration::from_millis(delay_ms));
        }
        device_vq.device_complete();
        received += 1;
    }
    println!("[device] done. received={}", received);
}

fn run_driver(vq_ptr: *mut Virtqueue<QUEUE_SIZE>) {
    let pool_ptr = unsafe { (vq_ptr as *mut u8).add(VQ_BYTES) } as *mut Message;

    // free_next はプロセスのプライベートヒープに置く（共有不要）。
    let mut free_next = vec![0u16; QUEUE_SIZE];
    let mut driver_vq = DriverVirtq::new(vq_ptr, &mut free_next);

    let msg_size = mem::size_of::<Message>() as u32;
    let mut sent = 0usize;
    let mut reclaimed = 0usize;

    let mut rng = rand::thread_rng();
    while sent < MSG_COUNT || reclaimed < MSG_COUNT {
        while driver_vq.get_used_id().is_some() {
            reclaimed += 1;
        }

        while sent < MSG_COUNT {
            let Some(id) = driver_vq.alloc_id() else {
                println!("[driver pid={}] queue full! waiting for device (sent={} reclaimed={})", nix::unistd::getpid(), sent, reclaimed);
                break;
            };
            let slot = unsafe { &mut *pool_ptr.add(id as usize) };
            *slot = Message {
                seq: sent as u32,
                value: sent as f64 * 0.1,
            };
            let addr = slot as *const Message as u64;
            println!(
                "[driver pid={}] send #{}: seq={} value={:.1}",
                nix::unistd::getpid(),
                sent,
                slot.seq,
                slot.value
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

    println!("[driver] done. sent={} reclaimed={}", sent, reclaimed);
}

fn main() {
    let fd = shm_open(
        SHM_NAME,
        OFlag::O_CREAT | OFlag::O_RDWR,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .expect("shm_open failed");

    ftruncate(&fd, SHM_SIZE as nix::libc::off_t).expect("ftruncate failed");

    let base = unsafe {
        mmap(
            None,
            NonZeroUsize::new(SHM_SIZE).unwrap(),
            ProtFlags::PROT_READ | ProtFlags::PROT_WRITE,
            MapFlags::MAP_SHARED,
            &fd,
            0,
        )
        .expect("mmap failed")
    };

    // 共有メモリをゼロ初期化（flags=0 → 全スロット「未使用」）
    unsafe {
        std::ptr::write_bytes(base.as_ptr() as *mut u8, 0, SHM_SIZE);
    }

    let vq_ptr = base.as_ptr() as *mut Virtqueue<QUEUE_SIZE>;

    match unsafe { fork().expect("fork failed") } {
        ForkResult::Child => {
            run_device(vq_ptr);
            unsafe { munmap(base, SHM_SIZE).ok() };
            std::process::exit(0);
        }
        ForkResult::Parent { child } => {
            run_driver(vq_ptr);
            waitpid(child, None).expect("waitpid failed");
            unsafe { munmap(base, SHM_SIZE).ok() };
            shm_unlink(SHM_NAME).ok();
        }
    }
}
