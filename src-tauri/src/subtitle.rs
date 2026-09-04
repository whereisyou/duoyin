use std::path::Path;

/// 将字幕段写入 SRT 文件
pub async fn write_srt(segments: &[crate::types::Segment], path: &Path) -> Result<(), String> {
    let mut content = String::new();
    for seg in segments {
        let text = if seg.translated.is_empty() {
            &seg.text
        } else {
            &seg.translated
        };
        content.push_str(&format!(
            "{}\n{} --> {}\n{}\n\n",
            seg.idx + 1,
            fmt_time(seg.start),
            fmt_time(seg.end),
            text,
        ));
    }
    tokio::fs::write(path, content)
        .await
        .map_err(|e| format!("write srt failed: {}", e))?;
    Ok(())
}

/// 格式化 SRT 时间戳 (hh:mm:ss,mmm)
pub fn fmt_time(s: f64) -> String {
    let h = (s / 3600.0) as u64;
    let m = ((s % 3600.0) / 60.0) as u64;
    let sec = s % 60.0;
    format!("{:02}:{:02}:{:06.3}", h, m, sec).replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Segment;

    #[test]
    fn test_fmt_time() {
        assert_eq!(fmt_time(0.0), "00:00:00,000");
        assert_eq!(fmt_time(1.5), "00:00:01,500");
        assert_eq!(fmt_time(60.0), "00:01:00,000");
        assert_eq!(fmt_time(3661.123), "01:01:01,123");
    }

    #[tokio::test]
    async fn test_write_srt() {
        let dir = std::env::temp_dir().join("videotrans_test_srt");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test.srt");

        let segments = vec![
            Segment {
                idx: 0,
                start: 0.0,
                end: 2.5,
                text: "你好世界".into(),
                translated: "Hello World".into(),
            },
            Segment {
                idx: 1,
                start: 3.0,
                end: 5.0,
                text: "这是测试".into(),
                translated: "This is a test".into(),
            },
        ];

        write_srt(&segments, &path).await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.contains("Hello World"));
        assert!(content.contains("00:00:00,000 --> 00:00:02,500"));
        assert!(content.contains("This is a test"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }

    #[tokio::test]
    async fn test_write_srt_no_translation() {
        let dir = std::env::temp_dir().join("videotrans_test_srt2");
        let _ = tokio::fs::create_dir_all(&dir).await;
        let path = dir.join("test2.srt");

        let segments = vec![Segment {
            idx: 0,
            start: 0.0,
            end: 1.0,
            text: "原始文本".into(),
            translated: String::new(),
        }];

        write_srt(&segments, &path).await.unwrap();

        let content = tokio::fs::read_to_string(&path).await.unwrap();
        // 没有翻译时，使用原文
        assert!(content.contains("原始文本"));
        assert!(!content.contains("translated"));

        let _ = tokio::fs::remove_dir_all(&dir).await;
    }
}
