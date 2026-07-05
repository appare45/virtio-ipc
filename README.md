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
| `Virtqueue<N>` | 共有メモリレイアウト（Descriptor Area + Device Area + Driver Area） |
| `VirtqDesc` | デスクリプタ 1 エントリ（addr / len / id / flags） |
| `EventSuppress` | 通知抑制構造体（§2.8.14） |
| `DriverVirtq::new(vq, free_next)` | ドライバ側キューの初期化 |
| `DriverVirtq::alloc_id()` | 空きバッファ id を確保（満杯なら `None`） |
| `DriverVirtq::place_buffer(id, addr, len, writable)` | バッファを available として公開 |
| `DriverVirtq::get_used_id()` | used 済みバッファの id を回収（ノンブロッキング） |
| `DeviceVirtq::new(vq)` | デバイス側キューの初期化 |
| `DeviceVirtq::device_take_available()` | 次の available デスクリプタを取得（ノンブロッキング） |
| `DeviceVirtq::device_complete()` | 処理完了を通知し used 状態にする |

## 使い方

```rust
use virtio_ipc::{EventSuppress, Virtqueue, VirtqDesc, driver::DriverVirtq, device::DeviceVirtq};

const QUEUE_SIZE: usize = 64;

// Virtqueue（Descriptor Area + Device/Driver Area）をヒープに確保
let suppress = EventSuppress { desc: 0, flags: 0 };
let mut vq: Box<Virtqueue<QUEUE_SIZE>> = Box::new(Virtqueue {
    desc_ring: [VirtqDesc { addr: 0, len: 0, id: 0, flags: 0 }; QUEUE_SIZE],
    device_event_suppress: suppress,
    driver_event_suppress: suppress,
});

// バッファプールを一括確保（id = インデックス、使い回す）
let mut pool: Vec<MyBuffer> = vec![MyBuffer::default(); QUEUE_SIZE];

// ドライバ側とデバイス側で同じ Virtqueue を共有
let vq_ptr = &mut *vq as *mut Virtqueue<QUEUE_SIZE>;
let mut free_next = [0u16; QUEUE_SIZE];
let mut driver_vq = DriverVirtq::new(vq_ptr, &mut free_next);
let mut device_vq = DeviceVirtq::new(vq_ptr);

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
| `Virtqueue<N>` 全体のレイアウト | §2.8 — Descriptor Area / Device Area / Driver Area の 3 パート構成 |
| `VirtqDesc` | §2.8.13 — pvirtq_desc（addr / len / id / flags） |
| `EventSuppress` | §2.8.14 — pvirtq_event_suppress |
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
- プロセス間で使う場合は共有メモリ（`mmap` など）上に `Virtqueue<N>` を配置してください（[`examples/process_send_recv.rs`](examples/process_send_recv.rs) 参照）。
- `DriverVirtq` は `unsafe` な生ポインタを内部で保持します。ライフタイム管理は呼び出し側の責任です。

## 今後の課題

### `flags` フィールドのアトミック化（UB 解消・最優先）

現在 `VirtqDesc.flags` は通常の `u16` フィールドであり、ドライバスレッドとデバイススレッドが同時に読み書きするためデータ競合（未定義動作）が生じる。`fence` は操作の順序を与えるが、アクセス自体のアトミック性は保証しない。

改善案: `VirtqDesc.flags` を `AtomicU16` に変更し、ペイロード（addr/len/id）を書いた後に `flags.store(new_flags, Ordering::Release)`、読み側は `flags.load(Ordering::Acquire)` とする。これはフラグをメモリバリアとして兼用する VirtIO 仕様 §2.8.1 の意図にも合致する。

### `DeviceVirtq.wrap` の変数名の明確化

`DeviceVirtq` 内の `wrap` フィールドは available 判定に Driver Ring Wrap Counter 相当の値として使用しているが、コメントでは「Device Ring Wrap Counter」と記載されており誤解を招く。`avail_wrap` など仕様用語と対応した名前に改めると正確になる。

### `&self` から `&mut VirtqDesc` を生成する API の整理

`desc_at(&self)` が `&mut VirtqDesc` を返す設計は aliasing 規則上グレーな領域。`AtomicU16` 化と合わせて、フラグは `AtomicU16::load` / `store` で直接アクセスし、`&mut` の生成を避ける形に整理する。

### Multi-descriptor チェーン対応（§2.8.5）

現状は 1 バッファ = 1 descriptor の前提。仕様 §2.8.5 の `VIRTQ_DESC_F_NEXT` を使った複数 descriptor によるバッファチェーンには未対応。

## ライセンス

MIT
