/// VirtIO共有メモリ vs Unix socket のプロセス間通信ベンチマーク
///
/// 同一メッセージ数を両方式で送受信し、スループットと所要時間を比較する。
///
/// レイアウト（VirtIO用共有メモリ内）:
///   [0 .. VQ_BYTES)   : Virtqueue<QUEUE_SIZE>
///   [VQ_BYTES .. ..)  : Message × QUEUE_SIZE  (メッセージプール)
use std::io::{Read, Write};
use std::mem;
use std::num::NonZeroUsize;
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::time::{Duration, Instant};

use nix::sys::mman::{MapFlags, ProtFlags, mmap, munmap, shm_open, shm_unlink};
use nix::sys::stat::Mode;
use nix::sys::wait::waitpid;
use nix::unistd::{ForkResult, fork, ftruncate};
use nix::fcntl::OFlag;
use virtio_ipc::{Virtqueue, device::DeviceVirtq, driver::DriverVirtq};

const QUEUE_SIZE: usize = 256;
const MSG_COUNT: usize = 100_000;
const SHM_NAME: &str = "/virtio_ipc_bench";
const UNIX_SOCKET_PATH: &str = "/tmp/virtio_ipc_bench.sock";

#[repr(C)]
#[derive(Clone, Copy)]
struct Message {
    seq: u32,
    payload: [u8; 60],
}

impl Default for Message {
    fn default() -> Self {
        Self { seq: 0, payload: [0u8; 60] }
    }
}

const VQ_BYTES: usize = mem::size_of::<Virtqueue<QUEUE_SIZE>>();
const POOL_BYTES: usize = mem::size_of::<Message>() * QUEUE_SIZE;
const SHM_SIZE: usize = VQ_BYTES + POOL_BYTES;
const MSG_SIZE: usize = mem::size_of::<Message>();

// ─── VirtIO ベンチマーク ────────────────────────────────────────────────────

fn virtio_device(vq_ptr: *mut Virtqueue<QUEUE_SIZE>) -> u64 {
    let mut device_vq = DeviceVirtq::new(vq_ptr);
    let pool_ptr = unsafe { (vq_ptr as *mut u8).add(VQ_BYTES) } as *const Message;

    let mut received = 0u64;
    while received < MSG_COUNT as u64 {
        let Some((_addr, _len, id)) = device_vq.device_take_available() else {
            std::hint::spin_loop();
            continue;
        };
        // シーケンス番号だけ読む（コンパイラ最適化による除去を防ぐ）
        let _seq = unsafe { (*pool_ptr.add(id as usize)).seq };
        std::hint::black_box(_seq);
        device_vq.device_complete();
        received += 1;
    }
    received
}

fn virtio_driver(vq_ptr: *mut Virtqueue<QUEUE_SIZE>) -> Duration {
    let pool_ptr = unsafe { (vq_ptr as *mut u8).add(VQ_BYTES) } as *mut Message;
    let mut free_next = vec![0u16; QUEUE_SIZE];
    let mut driver_vq = DriverVirtq::new(vq_ptr, &mut free_next);
    let msg_size = MSG_SIZE as u32;

    let mut sent = 0usize;
    let mut reclaimed = 0usize;

    let start = Instant::now();
    while sent < MSG_COUNT || reclaimed < MSG_COUNT {
        while driver_vq.get_used_id().is_some() {
            reclaimed += 1;
        }
        while sent < MSG_COUNT {
            let Some(id) = driver_vq.alloc_id() else { break };
            let slot = unsafe { &mut *pool_ptr.add(id as usize) };
            slot.seq = sent as u32;
            let addr = slot as *const Message as u64;
            driver_vq.place_buffer(id, addr, msg_size, false);
            sent += 1;
        }
        std::hint::spin_loop();
    }
    start.elapsed()
}

fn bench_virtio() -> Duration {
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
    unsafe { std::ptr::write_bytes(base.as_ptr() as *mut u8, 0, SHM_SIZE) };
    let vq_ptr = base.as_ptr() as *mut Virtqueue<QUEUE_SIZE>;

    let elapsed = match unsafe { fork().expect("fork failed") } {
        ForkResult::Child => {
            virtio_device(vq_ptr);
            unsafe { munmap(base, SHM_SIZE).ok() };
            std::process::exit(0);
        }
        ForkResult::Parent { child } => {
            let elapsed = virtio_driver(vq_ptr);
            waitpid(child, None).expect("waitpid failed");
            elapsed
        }
    };

    unsafe { munmap(base, SHM_SIZE).ok() };
    shm_unlink(SHM_NAME).ok();
    elapsed
}

// ─── Unix socket ベンチマーク ────────────────────────────────────────────────

fn unix_socket_receiver() {
    let listener = UnixListener::bind(UNIX_SOCKET_PATH).expect("bind failed");
    let (mut stream, _) = listener.accept().expect("accept failed");

    let mut buf = [0u8; MSG_SIZE];
    let mut received = 0u64;
    while received < MSG_COUNT as u64 {
        stream.read_exact(&mut buf).expect("read failed");
        let seq = u32::from_le_bytes(buf[..4].try_into().unwrap());
        std::hint::black_box(seq);
        received += 1;
    }
}

fn unix_socket_sender() -> Duration {
    // receiver が listen するまで少し待つ
    std::thread::sleep(Duration::from_millis(50));

    let mut stream = std::os::unix::net::UnixStream::connect(UNIX_SOCKET_PATH)
        .expect("connect failed");

    let msg = Message::default();
    let buf: &[u8] = unsafe {
        std::slice::from_raw_parts(&msg as *const Message as *const u8, MSG_SIZE)
    };

    let start = Instant::now();
    for seq in 0..MSG_COUNT {
        let mut m = msg;
        m.seq = seq as u32;
        let send_buf: &[u8] = unsafe {
            std::slice::from_raw_parts(&m as *const Message as *const u8, MSG_SIZE)
        };
        stream.write_all(send_buf).expect("write failed");
        let _ = buf; // suppress unused
    }
    start.elapsed()
}

fn bench_unix_socket() -> Duration {
    if Path::new(UNIX_SOCKET_PATH).exists() {
        std::fs::remove_file(UNIX_SOCKET_PATH).ok();
    }

    let elapsed = match unsafe { fork().expect("fork failed") } {
        ForkResult::Child => {
            unix_socket_receiver();
            std::process::exit(0);
        }
        ForkResult::Parent { child } => {
            let elapsed = unix_socket_sender();
            waitpid(child, None).expect("waitpid failed");
            elapsed
        }
    };

    std::fs::remove_file(UNIX_SOCKET_PATH).ok();
    elapsed
}

// ─── main ────────────────────────────────────────────────────────────────────

fn print_result(label: &str, elapsed: Duration, count: usize) {
    let secs = elapsed.as_secs_f64();
    let throughput = count as f64 / secs;
    let latency_us = secs * 1_000_000.0 / count as f64;
    println!(
        "{:<10}  {:>8.3} ms  {:>12.0} msg/s  {:>8.3} µs/msg",
        label,
        elapsed.as_secs_f64() * 1000.0,
        throughput,
        latency_us,
    );
}

fn main() {
    println!("=== IPC Benchmark: VirtIO shared-memory vs Unix socket ===");
    println!("messages: {}  msg_size: {} bytes  queue_size: {}", MSG_COUNT, MSG_SIZE, QUEUE_SIZE);
    println!();
    println!("{:<10}  {:>11}  {:>14}  {:>13}", "method", "total_time", "throughput", "avg_latency");
    println!("{}", "-".repeat(60));

    // VirtIOを先に計測
    let virtio_elapsed = bench_virtio();
    print_result("VirtIO", virtio_elapsed, MSG_COUNT);

    // Unix socketを計測
    let unix_socket_elapsed = bench_unix_socket();
    print_result("Unix socket", unix_socket_elapsed, MSG_COUNT);

    println!("{}", "-".repeat(60));
    let ratio = unix_socket_elapsed.as_secs_f64() / virtio_elapsed.as_secs_f64();
    println!("VirtIO speedup: {:.2}x faster than Unix socket", ratio);
}
