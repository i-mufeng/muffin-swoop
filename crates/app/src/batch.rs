//! 批量 URL：有限展开、规范化与稳定去重。预检成功前不创建任务。

use std::collections::HashSet;

pub const DEFAULT_BATCH_LIMIT: usize = 1000;
pub const MAX_BATCH_LIMIT: usize = 10_000;

pub fn parse(input: &str, limit: usize) -> Result<Vec<String>, String> {
    let limit = limit.clamp(1, MAX_BATCH_LIMIT);
    if input.len() > 2 * 1024 * 1024 {
        return Err(swoop_i18n::tr!("链接列表超过 2 MB，请分批添加").into());
    }
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    let mut total_bytes = 0;
    for (line, value) in input.lines().enumerate() {
        let value = value.trim();
        if value.is_empty() || value.starts_with('#') {
            continue;
        }
        let mut expanded = Vec::new();
        let mut expanded_bytes = 0;
        expand(value, limit, &mut expanded, &mut expanded_bytes, 0)
            .map_err(|e| swoop_i18n::trf!("第 {} 行：{e}", line + 1))?;
        for value in expanded {
            let mut url = url::Url::parse(&value).map_err(|_| swoop_i18n::trf!("第 {} 行不是有效链接", line + 1))?;
            if !matches!(url.scheme(), "http" | "https" | "ftp" | "ftps") || url.host_str().is_none() {
                return Err(swoop_i18n::trf!("第 {} 行仅支持 HTTP(S)/FTP(S) 链接", line + 1));
            }
            if matches!(url.scheme(), "ftp" | "ftps") {
                swoop_core::FtpTaskOptions::for_url(&value)
                    .map_err(|_| swoop_i18n::trf!("第 {} 行 FTP 链接无效；用户名密码请在登录栏填写", line + 1))?;
            }
            url.set_fragment(None);
            let value = url.to_string();
            if seen.insert(value.clone()) {
                total_bytes += value.len();
                if total_bytes > 2 * 1024 * 1024 {
                    return Err(swoop_i18n::tr!("展开后的链接列表超过 2 MB，请分批添加").into());
                }
                if result.len() == limit {
                    return Err(swoop_i18n::trf!("本次最多添加 {limit} 个链接，可在设置中调整"));
                }
                result.push(value);
            }
        }
    }
    if result.is_empty() {
        return Err(swoop_i18n::tr!("请至少输入一个链接").into());
    }
    Ok(result)
}

fn expand(value: &str, limit: usize, output: &mut Vec<String>, bytes: &mut usize, depth: usize) -> Result<(), String> {
    if value.len() > 16 * 1024 {
        return Err(swoop_i18n::tr!("单个链接不能超过 16 KB").into());
    }
    if depth > 8 {
        return Err(swoop_i18n::tr!("单个链接最多包含 8 个范围").into());
    }
    if output.len() >= limit {
        return Err(swoop_i18n::trf!("范围展开超过 {limit} 个链接，请缩小范围"));
    }
    let Some(start) = value.find('{') else {
        *bytes += value.len();
        if *bytes > 2 * 1024 * 1024 {
            return Err(swoop_i18n::tr!("展开后的链接列表超过 2 MB").into());
        }
        output.push(value.to_string());
        return Ok(());
    };
    let end = value[start..].find('}').map(|n| start + n).ok_or(swoop_i18n::tr!("范围缺少右花括号"))?;
    let (a, b) = value[start + 1..end].split_once("..").ok_or(swoop_i18n::tr!("范围格式为 {001..010} 或 {a..z}"))?;
    let numeric = !a.is_empty() && !b.is_empty() && a.bytes().chain(b.bytes()).all(|c| c.is_ascii_digit());
    let (first, last, width) = if numeric {
        let first = a.parse::<u64>().map_err(|_| swoop_i18n::tr!("范围起点过大"))?;
        let last = b.parse::<u64>().map_err(|_| swoop_i18n::tr!("范围终点过大"))?;
        let width = if a.starts_with('0') || b.starts_with('0') { a.len().max(b.len()) } else { 0 };
        (first, last, width)
    } else if a.len() == 1
        && b.len() == 1
        && ((a.as_bytes()[0].is_ascii_lowercase() && b.as_bytes()[0].is_ascii_lowercase())
            || (a.as_bytes()[0].is_ascii_uppercase() && b.as_bytes()[0].is_ascii_uppercase()))
    {
        (a.as_bytes()[0] as u64, b.as_bytes()[0] as u64, 0)
    } else {
        return Err(swoop_i18n::tr!("范围两端须为非负整数或同大小写的英文字母").into());
    };
    let count = first.abs_diff(last).checked_add(1).ok_or(swoop_i18n::tr!("范围过大"))?;
    if count > limit as u64 {
        return Err(swoop_i18n::trf!("范围展开超过 {limit} 个链接，请缩小范围"));
    }
    for i in 0..count {
        let n = if first <= last { first + i } else { first - i };
        let middle = if numeric { format!("{n:0width$}") } else { char::from(n as u8).to_string() };
        expand(&format!("{}{middle}{}", &value[..start], &value[end + 1..]), limit, output, bytes, depth + 1)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_padded_numbers_letters_and_descending_ranges() {
        assert_eq!(
            parse("https://example.com/{03..01}/{a..b}.zip", 6).unwrap(),
            [
                "https://example.com/03/a.zip",
                "https://example.com/03/b.zip",
                "https://example.com/02/a.zip",
                "https://example.com/02/b.zip",
                "https://example.com/01/a.zip",
                "https://example.com/01/b.zip",
            ]
        );
    }

    #[test]
    fn stable_deduplication_removes_fragments_but_keeps_queries() {
        assert_eq!(
            parse("# list\nhttps://example.com/a#one\nhttps://example.com/a#two\nhttps://example.com/a?q=1", 2)
                .unwrap(),
            ["https://example.com/a", "https://example.com/a?q=1"]
        );
    }

    #[test]
    fn rejects_invalid_rows_without_partial_results() {
        for text in [
            "",
            "file:///tmp/a",
            "javascript:alert(1)",
            "https://ok.test/a\nbad",
            "https://a.test/{1..x}",
            "https://a.test/{1..2",
            "ftp://user:secret@one.test/file",
            "ftp://one.test/dir/",
        ] {
            assert!(parse(text, 20).is_err(), "{text}");
        }
    }

    #[test]
    fn expands_ftp_and_ftps_without_changing_protocol() {
        assert_eq!(
            parse("ftp://one.test/{1..2}.zip\nftps://one.test/three.zip", 3).unwrap(),
            ["ftp://one.test/1.zip", "ftp://one.test/2.zip", "ftps://one.test/three.zip"]
        );
    }

    #[test]
    fn stops_cartesian_and_numeric_expansion_at_the_configured_limit() {
        for text in ["https://a.test/{1..999999999999999999}", "https://a.test/{1..10}/{1..10}"] {
            assert!(parse(text, 20).is_err());
        }
        assert!(parse("https://a.test/{1..20}", 20).is_ok());
        assert!(parse("https://a.test/{0..18446744073709551615}", 20).is_err());
        assert!(parse(&format!("https://a.test/{{{}..2}}", "0".repeat(20_000)), 20).is_err());
    }
}
