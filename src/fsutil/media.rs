//! Physical media detection for `optimize`: SSD → TRIM, HDD → defrag,
//! unknown → do nothing (never defrag an SSD).

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MediaType {
    Ssd,
    Hdd,
    #[default]
    Unknown,
}

impl MediaType {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaType::Ssd => "ssd",
            MediaType::Hdd => "hdd",
            MediaType::Unknown => "unknown",
        }
    }

    pub fn actionable(self) -> bool {
        !matches!(self, MediaType::Unknown)
    }
}

pub fn parse_rotational(text: &str) -> MediaType {
    match text.trim() {
        "0" => MediaType::Ssd,
        "1" => MediaType::Hdd,
        _ => MediaType::Unknown,
    }
}

pub fn parent_block_device(source: &str) -> Option<String> {
    let dev = source.strip_prefix("/dev/")?;
    if dev.starts_with("mapper/") || dev.starts_with("dm-") {
        return None;
    }
    let base = strip_partition(dev);
    if base.is_empty() {
        return None;
    }
    Some(base.to_string())
}

fn strip_partition(dev: &str) -> &str {
    let p_separated = dev.starts_with("nvme") || dev.starts_with("mmcblk");
    if p_separated {
        if let Some((head, tail)) = dev.rsplit_once('p') {
            let looks_like_partition =
                !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()) && head.ends_with(|c: char| c.is_ascii_digit());
            if looks_like_partition {
                return head;
            }
        }
        return dev;
    }
    dev.trim_end_matches(|c: char| c.is_ascii_digit())
}

pub fn device_for_mount(mounts: &str, mount: &str) -> Option<String> {
    let mut found = None;
    for line in mounts.lines() {
        let mut parts = line.split_whitespace();
        let (Some(source), Some(target)) = (parts.next(), parts.next()) else {
            continue;
        };
        if target == mount {
            found = Some(source.to_string());
        }
    }
    found
}

pub fn suggested_action(media: MediaType) -> &'static str {
    match media {
        MediaType::Ssd => "trim",
        MediaType::Hdd => "defrag",
        MediaType::Unknown => "none (unknown media — refusing)",
    }
}

#[cfg(unix)]
pub fn detect_for_mount(mount: &str) -> MediaType {
    let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
    let Some(source) = device_for_mount(&mounts, mount) else {
        return MediaType::Unknown;
    };
    let Some(device) = parent_block_device(&source) else {
        return MediaType::Unknown;
    };
    let rotational = std::fs::read_to_string(format!("/sys/block/{device}/queue/rotational")).unwrap_or_default();
    parse_rotational(&rotational)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rotational_maps_0_and_1() {
        assert_eq!(parse_rotational("0\n"), MediaType::Ssd);
        assert_eq!(parse_rotational("1"), MediaType::Hdd);
        assert_eq!(parse_rotational("junk"), MediaType::Unknown);
    }

    #[test]
    fn partition_suffixes_are_stripped_correctly() {
        assert_eq!(parent_block_device("/dev/sda1").unwrap(), "sda");
        assert_eq!(parent_block_device("/dev/nvme0n1p2").unwrap(), "nvme0n1");
        assert_eq!(parent_block_device("/dev/nvme0n1").unwrap(), "nvme0n1");
        assert_eq!(parent_block_device("/dev/mmcblk0").unwrap(), "mmcblk0");
        assert!(parent_block_device("/dev/mapper/vg0-root").is_none());
        assert!(parent_block_device("tmpfs").is_none());
    }

    #[test]
    fn unknown_media_is_not_actionable() {
        assert_eq!(suggested_action(MediaType::Ssd), "trim");
        assert_eq!(suggested_action(MediaType::Hdd), "defrag");
        assert!(!MediaType::Unknown.actionable());
    }
}
