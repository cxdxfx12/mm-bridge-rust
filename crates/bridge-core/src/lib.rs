//! bridge-core：转发桥的纯逻辑层，不依赖 Windows API、不依赖 GUI。
//!
//! 迁移约定（重要）：
//! 1. 每个模块都对应 `bridge/` 下的同名 Python 模块，行为必须**逐字节等价**；
//! 2. 每个模块都带 golden 测试，期望值由现网 Python 实现算出后写死在测试里，
//!    这样切换实现时任何漂移都会立刻被测出来；
//! 3. 系统相关能力（UIA / WGC / DPI / adb）不进本 crate，留在适配层。

pub mod data;
pub mod fingerprint;
