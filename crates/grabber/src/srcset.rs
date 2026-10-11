//! URL/descriptor tokenization follows HTML's image-candidate algorithm: commas
//! inside a URL (especially data: URLs) are not candidate separators.
use std::ops::Range;

fn space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

pub(crate) fn urls(value: &str) -> Vec<Range<usize>> {
    let bytes = value.as_bytes();
    let mut cursor = 0;
    let mut result = Vec::new();
    while cursor < bytes.len() {
        while cursor < bytes.len() && (space(bytes[cursor]) || bytes[cursor] == b',') {
            cursor += 1;
        }
        let start = cursor;
        while cursor < bytes.len() && !space(bytes[cursor]) {
            cursor += 1;
        }
        let mut end = cursor;
        while end > start && bytes[end - 1] == b',' {
            end -= 1;
        }
        if start == end {
            continue;
        }
        if end != cursor {
            result.push(start..end);
            continue;
        }
        let descriptor_start = cursor;
        let mut parentheses = 0u32;
        while cursor < bytes.len() {
            match bytes[cursor] {
                b'(' => parentheses = parentheses.saturating_add(1),
                b')' => parentheses = parentheses.saturating_sub(1),
                b',' if parentheses == 0 => break,
                _ => {}
            }
            cursor += 1;
        }
        if valid_descriptors(&value[descriptor_start..cursor]) {
            result.push(start..end);
        }
        if cursor < bytes.len() {
            cursor += 1;
        }
    }
    result
}

fn valid_descriptors(value: &str) -> bool {
    let mut width = false;
    let mut density = false;
    let mut height = false;
    for descriptor in value
        .split(|c: char| c.is_ascii() && space(c as u8))
        .filter(|s| !s.is_empty())
    {
        if let Some(number) = descriptor.strip_suffix('w') {
            if width || density || !positive_integer(number) {
                return false;
            }
            width = true;
        } else if let Some(number) = descriptor.strip_suffix('h') {
            if height || density || !positive_integer(number) {
                return false;
            }
            height = true;
        } else if let Some(number) = descriptor.strip_suffix('x') {
            if width
                || height
                || density
                || number.starts_with('+')
                || !number
                    .parse::<f64>()
                    .is_ok_and(|n| n.is_finite() && n >= 0.0)
            {
                return false;
            }
            density = true;
        } else {
            return false;
        }
    }
    !height || width
}

fn positive_integer(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|b| b.is_ascii_digit())
        && value.parse::<u64>().is_ok_and(|n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_commas_and_descriptors_preserve_candidate_boundaries() {
        let input = " data:image/svg+xml,%3Csvg%3E,a,b 1x, /two.png?q=1,2 2x, /three.png 640w, /none.png, /bad.png 1x 2x, /paren.png weird(a,b), /last.png 3x";
        let parts = urls(input)
            .into_iter()
            .map(|range| &input[range])
            .collect::<Vec<_>>();
        assert_eq!(
            parts,
            [
                "data:image/svg+xml,%3Csvg%3E,a,b",
                "/two.png?q=1,2",
                "/three.png",
                "/none.png",
                "/last.png"
            ]
        );
    }
}
