use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct AlistPath {
    pub server_url: String,
    pub base_path: String,
    pub full_path: String,
    pub name: String,
    pub size: u64,
    #[allow(dead_code)]
    pub is_dir: bool,
    pub sign: String,
}

impl AlistPath {
    pub fn new(
        server_url: impl Into<String>,
        base_path: impl Into<String>,
        parent_dir: &str,
        name: impl Into<String>,
        size: u64,
        is_dir: bool,
        sign: impl Into<String>,
    ) -> Self {
        let name_str = name.into();
        let full_path = join_alist_path(parent_dir, &name_str);
        Self {
            server_url: server_url.into(),
            base_path: base_path.into(),
            full_path,
            name: name_str,
            size,
            is_dir,
            sign: sign.into(),
        }
    }

    /// 文件的扩展名（包含小写前导点，如 ".mp4"）
    pub fn extension_lower(&self) -> String {
        let name_lower = self.name.to_lowercase();
        if let Some(pos) = name_lower.rfind('.') {
            name_lower[pos..].to_string()
        } else {
            String::new()
        }
    }

    /// 生成适配 302 挂载（如百度网盘）的 AList 直链播放地址
    /// 格式：{server_url}/d{base_path}{full_path}?sign={sign}
    pub fn alist_play_url(&self) -> String {
        let abs_path = format!(
            "{}{}",
            self.base_path.trim_end_matches('/'),
            ensure_leading_slash(&self.full_path)
        );

        let mut url = format!(
            "{}/d{}",
            self.server_url.trim_end_matches('/'),
            encode_path_segments(&abs_path)
        );

        if !self.sign.trim().is_empty() {
            url.push_str("?sign=");
            url.push_str(self.sign.trim());
        }

        url
    }

    /// 判断是否为蓝光原盘 (BDMV) 内的视频流文件
    pub fn is_bdmv_stream_file(&self) -> bool {
        let upper = self.full_path.to_uppercase();
        upper.contains("/BDMV/STREAM/") && (upper.ends_with(".M2TS") || upper.ends_with(".SSIF"))
    }

    /// 提取 BDMV 根目录路径（截取至 /BDMV/ 之前）
    pub fn bdmv_root_dir(&self) -> Option<String> {
        let upper = self.full_path.to_uppercase();
        if let Some(pos) = upper.find("/BDMV/") {
            let root = self.full_path[..pos].to_string();
            if !root.is_empty() {
                return Some(root);
            }
        }
        None
    }

    /// 计算对应的本地 .strm 目标保存路径
    pub fn local_strm_path(
        &self,
        source_dir: &str,
        target_dir: &Path,
        flatten: bool,
        is_bdmv_main: bool,
    ) -> PathBuf {
        if is_bdmv_main {
            // 蓝光原盘：以 BDMV 根目录名为影片名生成单个 .strm
            if let Some(root) = self.bdmv_root_dir() {
                let movie_name = root.trim_end_matches('/').rsplit('/').next().unwrap_or("Movie");
                let relative_dir = if flatten {
                    PathBuf::new()
                } else {
                    let source = source_dir.trim_end_matches('/');
                    let rel = root.strip_prefix(source).unwrap_or(&root).trim_start_matches('/');
                    // 取父级目录，避免多一层
                    let p = Path::new(rel);
                    p.parent().unwrap_or(p).to_path_buf()
                };
                return target_dir.join(relative_dir).join(format!("{}.strm", movie_name));
            }
        }

        let strm_filename = replace_extension_with_strm(&self.name);

        if flatten {
            target_dir.join(strm_filename)
        } else {
            let source = source_dir.trim_end_matches('/');
            let rel_path_str = self
                .full_path
                .strip_prefix(source)
                .unwrap_or(&self.full_path)
                .trim_start_matches('/');

            let rel_path = Path::new(rel_path_str);
            let parent_rel = rel_path.parent().unwrap_or_else(|| Path::new(""));
            target_dir.join(parent_rel).join(strm_filename)
        }
    }
}

pub fn join_alist_path(parent: &str, name: &str) -> String {
    let p = parent.trim_end_matches('/');
    if p.is_empty() {
        format!("/{}", name)
    } else {
        format!("{}/{}", p, name)
    }
}

pub fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    }
}

/// 对路径段进行 URL 百分比编码，但保留 '/' 字符
pub fn encode_path_segments(path: &str) -> String {
    path.split('/')
        .map(urlencoding::encode)
        .collect::<Vec<_>>()
        .join("/")
}

pub fn replace_extension_with_strm(filename: &str) -> String {
    if let Some(pos) = filename.rfind('.') {
        format!("{}.strm", &filename[..pos])
    } else {
        format!("{}.strm", filename)
    }
}

/// 是否为需要忽略的系统无用文件
pub fn is_junk_file(path: &str) -> bool {
    path.contains("@eaDir")
        || path.contains("Thumbs.db")
        || path.contains(".DS_Store")
        || path.contains(".git")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_alist_path() {
        assert_eq!(join_alist_path("/", "test.mp4"), "/test.mp4");
        assert_eq!(join_alist_path("/movie", "test.mp4"), "/movie/test.mp4");
        assert_eq!(join_alist_path("/movie/", "test.mp4"), "/movie/test.mp4");
    }

    #[test]
    fn test_alist_play_url_generation() {
        let p = AlistPath::new(
            "http://127.0.0.1:5244",
            "/",
            "/百度网盘/电影",
            "奥本海默.2023.mp4",
            1000,
            false,
            "signature_token_abc:123",
        );
        let url = p.alist_play_url();
        assert_eq!(
            url,
            "http://127.0.0.1:5244/d/%E7%99%BE%E5%BA%A6%E7%BD%91%E7%9B%98/%E7%94%B5%E5%BD%B1/%E5%A5%A5%E6%9C%AC%E6%B5%B7%E9%BB%98.2023.mp4?sign=signature_token_abc:123"
        );
    }

    #[test]
    fn test_bdmv_detection() {
        let p = AlistPath::new(
            "http://127.0.0.1:5244",
            "/",
            "/Movies/Avatar/BDMV/STREAM",
            "00001.m2ts",
            50000000,
            false,
            "",
        );
        assert!(p.is_bdmv_stream_file());
        assert_eq!(p.bdmv_root_dir(), Some("/Movies/Avatar".to_string()));

        let target = Path::new("C:/media");
        let strm_path = p.local_strm_path("/Movies", target, false, true);
        assert_eq!(strm_path, PathBuf::from("C:/media/Avatar.strm"));
    }
}
