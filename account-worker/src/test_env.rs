//! 测试用的全局锁。
//!
//! `std::env::set_var` 是进程级状态：并行跑的测试如果各自把 `AICC_QODER_APP_DIRS`
//! / `AICC_PS_BIN` 指向自己的临时目录，就会互相把对方"装好的客户端"换掉。
//! 凡是读写这些变量的测试都必须先拿这把锁，改完再放。
//!
//! 只用 `#[cfg(test)]` 编译，生产二进制里不存在。

use std::sync::{Mutex, MutexGuard};

static ENV_LOCK: Mutex<()> = Mutex::new(());

pub fn lock() -> MutexGuard<'static, ()> {
    // 一个测试断言失败时锁会被标记 poisoning；后面排队的人仍需继续跑，
    // 所以这里直接取回内部值，而不是把整组测试一起判死。
    ENV_LOCK.lock().unwrap_or_else(|err| err.into_inner())
}
