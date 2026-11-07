//! 从 Setup.exe 尾部的 PE overlay 读出内置转发引擎归档并解压。
//!
//! 打包端见 `rust/assets/pack_backend.py`。刻意不用 zip：自定义格式只要 ~120 行、
//! 两个纯 Rust crate（flate2 / crc32fast），就能边解压边报进度，也不会把
//! 200 MB 的归档在编译期塞进 rustc。

use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const FOOTER_SIZE: usize = 64;
const FOOTER_MAGIC: &[u8; 16] = b"MIAOMIAO-BUNDLE1";
const BUNDLE_MAGIC: &[u8; 8] = b"MMBUND01";
const HEADER_SIZE: usize = 28; // magic8 + count4 + total_raw8 + name_fp8
const MARKER: &str = ".bundle-crc";

pub struct Bundle {
    pub bytes: Vec<u8>,
    pub checksum: u32,
    pub count: u32,
    pub total_raw: u64,
    pub marker: String,
}

#[derive(Default, Debug)]
pub struct Stats {
    pub files: usize,
    pub bytes: u64,
    pub config_created: bool,
}

fn u32le(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64le(b: &[u8], o: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?))
}

fn read_footer(me: &Path) -> io::Result<Option<(u64, u32, u32, u64)>> {
    // 返回 (归档长度, crc32, 格式版本, 含 overlay 的文件总长)
    let mut f = File::open(me)?;
    let len = f.metadata()?.len();
    if len < FOOTER_SIZE as u64 + 64 {
        return Ok(None);
    }
    f.seek(SeekFrom::End(-(FOOTER_SIZE as i64)))?;
    let mut footer = [0u8; FOOTER_SIZE];
    f.read_exact(&mut footer)?;
    if !footer[..16].starts_with(FOOTER_MAGIC) {
        return Ok(None);
    }
    let size = u64le(&footer, 16).unwrap_or(0);
    let crc = u32le(&footer, 24).unwrap_or(0) as u32;
    let ver = u32le(&footer, 32).unwrap_or(0);
    if size == 0 || size > len - FOOTER_SIZE as u64 {
        return Err(io::Error::other("安装包尾注里的归档长度不合法，文件可能已损坏"));
    }
    Ok(Some((size, crc, ver, len)))
}

/// 读自身尾部的归档。debug 构建 / 尚未追加归档时返回 None（安装器退化为「仅界面」模式）。
pub fn read_own() -> io::Result<Option<Bundle>> {
    let me = std::env::current_exe()?;
    let Some((size, crc, ver, file_len)) = read_footer(&me)? else {
        return Ok(None);
    };
    if ver != 1 {
        return Err(io::Error::other(format!("不支持的归档格式版本 {ver}，请换用配套安装包")));
    }
    let start = file_len - FOOTER_SIZE as u64 - size;
    let mut f = File::open(&me)?;
    f.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0u8; size as usize];
    f.read_exact(&mut bytes)?;
    if crc32fast::hash(&bytes) != crc {
        return Err(io::Error::other("内置转发引擎校验失败，安装包可能下载不完整，请重新获取"));
    }
    if !bytes.starts_with(BUNDLE_MAGIC) {
        return Err(io::Error::other("内置转发引擎格式不正确"));
    }
    let count = u32le(&bytes, 8).ok_or_else(|| io::Error::other("归档头损坏"))?;
    let total_raw = u64le(&bytes, 12).unwrap_or(0);
    Ok(Some(Bundle {
        marker: format!("{:08x}-{}", crc, count),
        bytes,
        checksum: crc,
        count,
        total_raw,
    }))
}

/// 自身去掉 overlay 后的长度：卸载入口只需要这部分（约 10 MB），
/// 不必把 200 MB 的引擎再复制一份到临时目录。
pub fn stub_len() -> io::Result<Option<u64>> {
    let me = std::env::current_exe()?;
    let Some((size, _, _, file_len)) = read_footer(&me)? else {
        return Ok(None);
    };
    Ok(Some(file_len - FOOTER_SIZE as u64 - size))
}

/// 归档名 → 落盘位置。只接受两种前缀，杜绝路径穿越。
fn destination(dir: &Path, name: &str) -> io::Result<(PathBuf, bool)> {
    if name.is_empty()
        || name.contains('\\')
        || name.contains("..")
        || name.contains(':')
        || name.starts_with('/')
    {
        return Err(io::Error::other(format!("归档里有非法路径：{name}")));
    }
    if name == "config.yaml" {
        return Ok((dir.join("config.yaml"), true)); // 用户改过的配置不能被覆盖
    }
    if let Some(rest) = name.strip_prefix("backend/") {
        if rest.is_empty() {
            return Err(io::Error::other("归档里有空的 backend 路径"));
        }
        return Ok((dir.join("backend").join(rest), false));
    }
    Err(io::Error::other(format!("归档里有未识别的路径：{name}")))
}

fn write_entry(target: &Path, method: u8, data: &[u8], expected: u64) -> io::Result<u64> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = File::create(target)?;
    let n = if method == 0 {
        f.write_all(data)?;
        data.len() as u64
    } else if method == 1 {
        let mut dec = flate2::read::DeflateDecoder::new(data);
        io::copy(&mut dec, &mut f)?
    } else {
        return Err(io::Error::other(format!("未知压缩方式 {method}")));
    };
    if n != expected {
        return Err(io::Error::other(format!(
            "解压长度不符：{} 期望 {expected} 实得 {n}",
            target.display()
        )));
    }
    Ok(n)
}

/// 把归档解压到安装目录。progress(已完成, 总数) 用于安装提示。
pub fn extract(
    bundle: &Bundle,
    dir: &Path,
    progress: &mut impl FnMut(usize, usize),
) -> io::Result<Stats> {
    let b = &bundle.bytes;
    if b.len() < HEADER_SIZE {
        return Err(io::Error::other("归档头过短"));
    }
    let count = u32le(b, 8).ok_or_else(|| io::Error::other("归档头损坏"))? as usize;
    let mut pos = HEADER_SIZE;
    let mut st = Stats::default();

    for i in 0..count {
        let name_len = u16::from_le_bytes(
            b.get(pos..pos + 2)
                .and_then(|s| s.try_into().ok())
                .ok_or_else(|| io::Error::other(format!("第 {i} 项的归档名长度损坏")))?,
        ) as usize;
        pos += 2;
        let name = std::str::from_utf8(
            b.get(pos..pos + name_len)
                .ok_or_else(|| io::Error::other(format!("第 {i} 项的归档名越界")))?,
        )
        .map_err(|e| io::Error::other(format!("第 {i} 项归档名不是 UTF-8：{e}")))?
        .to_owned();
        pos += name_len;

        let method = *b
            .get(pos)
            .ok_or_else(|| io::Error::other(format!("第 {i} 项缺少压缩方式")))?;
        pos += 1;
        let raw_len = u64le(b, pos).ok_or_else(|| io::Error::other("归档长度字段越界"))?;
        pos += 8;
        let stored_len = u64le(b, pos).ok_or_else(|| io::Error::other("归档长度字段越界"))?;
        pos += 8;
        let data = b
            .get(pos..pos + stored_len as usize)
            .ok_or_else(|| io::Error::other(format!("{name} 的数据越界")))?;
        pos += stored_len as usize;

        let (target, protect_existing) = destination(dir, &name)?;
        if protect_existing && target.is_file() {
            // 升级时保留用户已经改过的 config.yaml
        } else {
            st.bytes += write_entry(&target, method, data, raw_len)?;
            st.files += 1;
            if protect_existing {
                st.config_created = true;
            }
        }
        if (i + 1) % 200 == 0 || i + 1 == count {
            progress(i + 1, count);
        }
    }
    // 标记本次归档版本，覆盖安装时若一致可跳过整段解压。
    let _ = fs::write(dir.join("backend").join(MARKER), bundle.marker.as_bytes());
    Ok(st)
}

/// 已经装过同样内容的引擎吗？（用于秒级覆盖安装）
pub fn already_installed(bundle: &Bundle, dir: &Path) -> bool {
    fs::read_to_string(dir.join("backend").join(MARKER))
        .map(|s| s == bundle.marker)
        .unwrap_or(false)
        && dir
            .join("backend")
            .join("runtime")
            .join("python.exe")
            .is_file()
        && dir
            .join("backend")
            .join("app")
            .join("bridge_worker.py")
            .is_file()
}

/// 覆盖安装前清掉旧引擎目录。worker 还在跑时会失败，这时给出可读提示。
pub fn clear_backend(dir: &Path) -> io::Result<()> {
    let p = dir.join("backend");
    if p.exists() {
        fs::remove_dir_all(&p).map_err(|e| {
            io::Error::other(format!(
                "无法更新内置转发引擎（{}）：{e}\n请先关闭 Rust 版窗口并停止转发，再重新安装。",
                p.display()
            ))
        })?;
    }
    Ok(())
}

pub fn describe(bundle: &Bundle) -> String {
    format!(
        "{} 个文件 / {:.1} MB",
        bundle.count,
        bundle.total_raw as f64 / 1048576.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_maps_bundle_names() {
        let dir = Path::new(r"C:\Programs\App");
        assert_eq!(
            destination(dir, "backend/runtime/python.exe").unwrap().0,
            dir.join("backend").join("runtime").join("python.exe")
        );
        assert_eq!(destination(dir, "config.yaml").unwrap().0, dir.join("config.yaml"));
        assert!(destination(dir, "config.yaml").unwrap().1);
    }

    #[test]
    fn destination_rejects_path_traversal() {
        let dir = Path::new(r"C:\Programs\App");
        for bad in ["../evil.py", "backend/../../evil.py", "C:/Windows/x", r"backend\..\x", "/abs"] {
            assert!(destination(dir, bad).is_err(), "{bad} 应该被拒绝");
        }
    }
}
