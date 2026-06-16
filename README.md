# virtio-ipc

VirtIO Packed Virtqueue（仕様 §2.8）をベースにした、スレッド間 IPC ライブラリです。
共有メモリ上のデスクリプタリングを使い、ロックフリーのスピンポーリングでバッファを受け渡します。

## 概要

VirtIO の Packed Virtqueue は、`AVAIL` / `USED` ビットと Ring Wrap Counter を組み合わせることで、
単一のデスクリプタ配列だけでキューの状態を管理します。このクレートはその仕組みを Rust で実装し、
スレッド間通信のプリミティブとして提供します。

```
Driver ──place_buffer()──▶ Descriptor Ring ──device_take_available()──▶ Device
                                                ◀─────device_complete()──────
Driver ◀──get_buffer()───
```

## 主な API

| 型 / 関数 | 役割 |
|---|---|
| `VirtqDesc` | デスクリプタ 1 エントリ（addr / len / id / flags） |
| `Virtq::new(desc, num)` | キューの初期化 |
| `Virtq::place_buffer(buf)` | ドライバ側: バッファを available に登録（スロットが空くまでスピン） |
| `Virtq::device_take_available()` | デバイス側: 次の available デスクリプタを取得（ノンブロッキング） |
| `Virtq::device_complete()` | デバイス側: 処理完了を通知し used 状態にする |
| `Virtq::get_buffer()` | ドライバ側: used 完了したバッファを回収 |

## 使い方

```rust
use virtio_ipc::{BufferElement, Virtq, VirtqDesc};

const QUEUE_SIZE: usize = 64;

// デスクリプタリングをヒープに確保
let mut desc_ring: Vec<VirtqDesc> = (0..QUEUE_SIZE)
    .map(|_| VirtqDesc { addr: 0, len: 0, id: 0, flags: 0 })
    .collect();

// ドライバ側キューとデバイス側キュー（同じリングを共有）
let mut driver_vq = Virtq::new(desc_ring.as_mut_ptr(), QUEUE_SIZE);
let mut device_vq = Virtq::new(desc_ring.as_mut_ptr(), QUEUE_SIZE);
```

詳細な使用例は [`examples/send_recv.rs`](examples/send_recv.rs) を参照してください。

## サンプルの実行

```sh
cargo run --example send_recv
```

100 万件のメッセージをドライバスレッドからデバイススレッドへ送受信し、スループットを確認できます。

## 仕様との対応

| 実装箇所 | VirtIO 仕様 |
|---|---|
| `is_free_slot_ready` | §2.8.22 — AVAIL == USED なら used 済み（再利用可） |
| `place_buffer` | §2.8.21 — AVAIL ビットを Driver Ring Wrap Counter に合わせる |
| `device_complete` | §2.8.22 — USED ビットを Device Ring Wrap Counter に合わせる |

## 注意事項

- 現在はスレッド間共有を想定しています。プロセス間で使う場合は共有メモリ（`mmap` など）上にデスクリプタリングを配置してください。
- `Virtq` は `unsafe` な生ポインタを内部で保持します。ライフタイム管理はユーザー側の責任です。

## ライセンス

MIT
