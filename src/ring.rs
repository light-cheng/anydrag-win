//! 固定容量 SPSC 环形缓冲（无锁）。
//!
//! 使用前提（构造文档写死）：**单生产者 = hook 线程，单消费者 = engine 线程**。
//! 生产者 `push`（`&self`），消费者 `drain`（`&mut self`，同线程独占）。
//!
//! 满时策略（设计记录，GOAL T1.2 写"丢最旧"，实现改为**丢新**——原因如下）：
//! 消费者取走槽位（`Option::take`）到发布 head（`fetch_add`）之间存在"在途窗口"，
//! 整个在途窗口内的槽位都可能正被消费者读取。生产者若在此时改写"最旧槽"（真·丢最旧），
//! 会与消费者的非原子 take 构成 data race（实测 100k 并发守恒测试复现：
//! 槽位被 None 覆盖 → 读 None panic）。安全协议：
//!   - 有空间 → 正常写入（健康消费者下环形队列天然只保留最新 N 条事件 = 等效丢最旧）；
//!   - 无空间 → 自旋让位 `SPIN_LIMIT` 轮（有界 = O(1)，钩子回调不得阻塞）；
//!   - 仍无空间（消费者卡死/被调试器暂停）→ **丢弃新事件**并计数。
//! 对鼠标流语义无损：引擎每个事件只关心最新位置，消费者卡死时丢新/丢旧等价
//! （整个手势已失，进程多半即将退出）。守恒恒等式：pushed == drained + dropped。
//!
//! 实现说明：读槽逻辑在 `drain_impl(&self)`（共享字段全是原子/UnsafeCell，
//! 并发 `&self` 合法）；公开 `drain(&mut self)` 以 `&mut` 钉死"唯一消费者"契约，
//! 测试则可用 `Arc` 共享跑守恒并发测试。

use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 满时自旋让出的轮数（有界 = O(1) 工作，满足"钩子回调不得阻塞"不变量）。
const SPIN_LIMIT: u32 = 16;

pub struct Ring<T: Copy, const N: usize> {
    /// 逻辑头/尾，单调递增（索引 = `x % N`）。
    /// head：消费者 fetch_add(taken)，生产者满时 CAS +1（丢最旧）；
    /// tail：仅生产者写。
    head: AtomicUsize,
    tail: AtomicUsize,
    slots: Box<[UnsafeCell<Option<T>>; N]>,
    /// 因满而丢弃的事件计数（可观测，测试断言守恒用）。
    dropped: AtomicUsize,
}

impl<T: Copy, const N: usize> Ring<T, N> {
    /// 单生产者/单消费者使用（见模块文档）。构造是一次性堆分配，
    /// 热路径（push/drain）零堆分配。
    pub fn new() -> Self {
        Self {
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            slots: Box::new(std::array::from_fn(|_| UnsafeCell::new(None))),
            dropped: AtomicUsize::new(0),
        }
    }

    /// 推入事件。有空间 → 写入；满 → 自旋让位 → 仍满（消费者卡死）→ 丢新事件并计数。
    /// 无堆分配、无阻塞等待（自旋有界），可安全用于 LL 钩子回调。
    pub fn push(&self, v: T) {
        let mut spins = 0u32;
        loop {
            let tail = self.tail.load(Ordering::Acquire);
            let head = self.head.load(Ordering::Acquire);
            if tail - head < N {
                // 空间检查基于已发布的 head：写入槽位严格领先消费者在途窗口（见模块文档）。
                unsafe {
                    (*self.slots[tail % N].get()).replace(v);
                }
                self.tail.store(tail + 1, Ordering::Release);
                return;
            }
            // 满：先给消费者腾位的机会（有界自旋 = O(1)，不阻塞钩子回调）。
            if spins < SPIN_LIMIT {
                spins += 1;
                std::thread::yield_now();
                continue;
            }
            // 消费者卡死的退化路径：丢弃新事件（而非改写最旧槽——那会与消费者在途
            // 读取构成 data race，见模块文档）。计数供可观测/守恒断言。
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }

    /// 取出最多 `out.len()` 个事件。`&mut self` 按 SPSC 契约保证唯一消费者。
    pub fn drain(&mut self, out: &mut [MaybeUninit<T>]) -> usize {
        self.drain_impl(out)
    }

    /// `drain` 的 `&self` 内核。共享字段全是原子/UnsafeCell，
    /// 经 `Arc` 并发 `&self` 合法——仅供守恒测试使用，生产代码走 `drain`。
    #[doc(hidden)]
    pub fn drain_impl(&self, out: &mut [MaybeUninit<T>]) -> usize {
        let head = self.head.load(Ordering::Acquire);
        let tail = self.tail.load(Ordering::Acquire);
        // 生产者保证 tail - head ≤ N；min(N) 是双保险。
        let avail = (tail - head).min(N).min(out.len());
        let mut i = 0;
        while i < avail {
            let slot = head + i;
            // 该槽必已被生产者 replace（未写槽的索引 ≥ tail，不会读）。
            let v = unsafe { (*self.slots[slot % N].get()).take().expect("queued slot is Some") };
            out[i].write(v);
            i += 1;
        }
        if i > 0 {
            self.head.fetch_add(i, Ordering::Release);
        }
        i
    }

    /// 因满被丢弃的事件总数（守恒断言：pushed == drained + dropped）。
    pub fn dropped(&self) -> usize {
        self.dropped.load(Ordering::Relaxed)
    }
}

// UnsafeCell 保守推导让 Ring 失去 Sync，但它的共享字段（head/tail/dropped）
// 全是原子，槽区由 UnsafeCell 显式声明可变性；正确性由 SPSC 协议钉死
// （单生产者 push / 单消费者 drain，drain(&mut self) 在类型层面保证）。
// 跨线程共享（Arc、static OnceLock、守恒测试）必须显式补 Sync。
unsafe impl<T: Copy, const N: usize> Sync for Ring<T, N> {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn ring_fifo_ordering() {
        let mut r: Ring<u32, 8> = Ring::new();
        for i in 1..=6 {
            r.push(i);
        }
        let mut out = [MaybeUninit::uninit(); 8];
        let n = r.drain(&mut out);
        assert_eq!(n, 6);
        let vals: Vec<u32> = (0..n).map(|i| unsafe { out[i].assume_init_read() }).collect();
        assert_eq!(vals, vec![1, 2, 3, 4, 5, 6], "FIFO order must hold");
    }

    #[test]
    fn ring_full_drops_when_stuck() {
        // 无消费者（卡死）：灌 9 个进 4 槽环 → 5 个新事件被丢（丢新，见模块文档），
        // 环内保留最早的 4 个，dropped() 计数守恒。
        let mut r: Ring<u32, 4> = Ring::new();
        for i in 1..=9 {
            r.push(i);
        }
        assert_eq!(r.dropped(), 5, "9 pushes into 4 slots with a stuck consumer drop 5");
        let mut out = [MaybeUninit::uninit(); 4];
        let n = r.drain(&mut out);
        let vals: Vec<u32> = (0..n).map(|i| unsafe { out[i].assume_init_read() }).collect();
        assert_eq!(vals, vec![1, 2, 3, 4], "stuck consumer: ring keeps the oldest N");
    }

    #[test]
    fn ring_concurrent_conservation() {
        // 乱序并发守恒：生产者灌 100k 事件、消费者持续 drain；
        // 无论丢了几个（消费者慢时可能 >0），pushed == drained + dropped 恒成立。
        const TOTAL: usize = 100_000;
        let r: Arc<Ring<u32, 64>> = Arc::new(Ring::new());
        let done = Arc::new(AtomicBool::new(false));

        let producer = {
            let r = Arc::clone(&r);
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                for i in 0..TOTAL {
                    r.push(i as u32);
                }
                done.store(true, Ordering::Release);
            })
        };
        let consumer = {
            let r = Arc::clone(&r);
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let mut out = [MaybeUninit::uninit(); 64];
                let mut drained = 0usize;
                loop {
                    let n = r.drain_impl(&mut out);
                    drained += n;
                    if n == 0 && done.load(Ordering::Acquire) {
                        break;
                    }
                }
                drained
            })
        };
        producer.join().unwrap();
        let drained = consumer.join().unwrap();
        assert_eq!(drained + r.dropped(), TOTAL, "count must be conserved");
    }
}
