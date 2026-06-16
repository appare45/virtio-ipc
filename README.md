# virtio-ipc

VirtIO Packed Virtqueue（仕様 §2.8）をベースにした、スレッド間 IPC ライブラリです。
共有メモリ上のデスクリプタリングを使い、ロックフリーのスピンポーリングでバッファを受け渡します。

## 概要

VirtIO の Packed Virtqueue は、`AVAIL` / `USED` ビットと Ring Wrap Counter を組み合わせることで、
単一のデスクリプタ配列だけでキューの状態を管理します。このクレートはその仕組みを Rust で実装し、
スレッド間通信のプリミティブとして提供します。

```
Driver ──alloc_id()──▶ [バッファ準備]
       ──place_buffer()──▶ Descriptor Ring ──device_take_available()──▶ Device
                                              ◀────device_complete()────
Driver ◀──get_used_id()──
```

バッファメモリはライブラリの外側で管理します。ドライバは `alloc_id` で空きスロット番号を取得し、
対応するバッファを準備してから `place_buffer` で公開します。`get_used_id` で返ってきた id の
バッファは再利用できます。

## 主な API

| 型 / 関数 | 役割 |
|---|---|
| `VirtqDesc` | デスクリプタ 1 エントリ（addr / len / id / flags） |
| `DriverVirtq::new(desc, num)` | ドライバ側キューの初期化 |
| `DriverVirtq::alloc_id()` | 空きバッファ id を確保（満杯なら `None`） |
| `DriverVirtq::place_buffer(id, addr, len, writable)` | バッファを available として公開 |
| `DriverVirtq::get_used_id()` | used 済みバッファの id を回収（ノンブロッキング） |
| `DeviceVirtq::new(desc, num)` | デバイス側キューの初期化 |
| `DeviceVirtq::device_take_available()` | 次の available デスクリプタを取得（ノンブロッキング） |
| `DeviceVirtq::device_complete()` | 処理完了を通知し used 状態にする |

## 使い方

```rust
use virtio_ipc::{DriverVirtq, DeviceVirtq, VirtqDesc};

const QUEUE_SIZE: usize = 64;

// デスクリプタリングをヒープに確保
let mut desc_ring: Vec<VirtqDesc> = (0..QUEUE_SIZE)
    .map(|_| VirtqDesc { addr: 0, len: 0, id: 0, flags: 0 })
    .collect();

// バッファプールを一括確保（id = インデックス、使い回す）
let mut pool: Vec<MyBuffer> = vec![MyBuffer::default(); QUEUE_SIZE];

// ドライバ側とデバイス側で同じリングを共有
let mut driver_vq = DriverVirtq::new(desc_ring.as_mut_ptr(), QUEUE_SIZE);
let mut device_vq = DeviceVirtq::new(desc_ring.as_mut_ptr(), QUEUE_SIZE);

// 送信（ドライバ側）
if let Some(id) = driver_vq.alloc_id() {
    pool[id as usize] = MyBuffer { ... };           // バッファに書く
    let addr = &pool[id as usize] as *const _ as u64;
    driver_vq.place_buffer(id, addr, size, false);  // available として公開
}

// 受信（デバイス側）
if let Some((addr, len, _id)) = device_vq.device_take_available() {
    let buf = unsafe { &*(addr as *const MyBuffer) };
    // ... 処理 ...
    device_vq.device_complete();
}

// 回収（ドライバ側）
if let Some(id) = driver_vq.get_used_id() {
    // pool[id] を再利用できる
}
```

詳細な使用例は [`examples/send_recv.rs`](examples/send_recv.rs) を参照してください。

## サンプルの実行

```sh
cargo run --example send_recv
```

100 万件のメッセージをドライバスレッドからデバイススレッドへ送受信し、動作を確認できます。

## 仕様との対応

| 実装箇所 | VirtIO 仕様 §2.8 |
|---|---|
| `VIRTQ_DESC_F_AVAIL` / `VIRTQ_DESC_F_USED` ビット | §2.8.1 — Driver/Device Ring Wrap Counter に合わせて設定 |
| `place_buffer` | §2.8.1 — available: AVAIL=driver_wrap, USED=逆（AVAIL ≠ USED） |
| `device_complete` | §2.8.1 — used: AVAIL == USED == device_wrap |
| `get_used_id` | §2.8.1 — AVAIL == USED == device_wrap を確認して回収 |
| `alloc_id` / `get_used_id` の id | §2.8.6 — Buffer ID によるバッファ識別 |

## 設計上の注意

- バッファメモリ（プール）はライブラリの外側で管理します。Virtq は addr/len だけを扱います。
  これは実ハードウェアでデスクリプタリングとバッファが別領域に置かれる構造に対応しています。
- `DriverVirtq` と `DeviceVirtq` はそれぞれ別スレッドで使用します。
  同期はデスクリプタの AVAIL/USED フラグとメモリバリア（`fence`）のみで行います。
- プロセス間で使う場合は共有メモリ（`mmap` など）上にデスクリプタリングを配置してください。
- `DriverVirtq` は `unsafe` な生ポインタを内部で保持します。ライフタイム管理は呼び出し側の責任です。

## ライセンス

MIT
