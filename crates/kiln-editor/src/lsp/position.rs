//! LSP 의 UTF-16 열과 줄 안 바이트·문자 열 사이 변환.

/// 줄 안 바이트 오프셋을 UTF-16 열로 바꾼다. 줄 끝을 넘으면 줄 끝, 문자 중간이면 그 문자 앞.
pub fn utf16_col(line: &str, byte: usize) -> u32 {
    let mut b = byte.min(line.len());
    while !line.is_char_boundary(b) {
        b -= 1;
    }
    line[..b].chars().map(char::len_utf16).sum::<usize>() as u32
}

/// UTF-16 열을 줄 안 바이트 오프셋으로 바꾼다. 줄 끝을 넘으면 줄 끝, 서로게이트 쌍 중간이면 그 문자 앞.
pub fn byte_col(line: &str, utf16: u32) -> usize {
    let target = utf16 as usize;
    let mut units = 0usize;
    for (i, c) in line.char_indices() {
        let next = units + c.len_utf16();
        if next > target {
            return i;
        }
        units = next;
    }
    line.len()
}

/// UTF-16 열을 문자(유니코드 스칼라) 열로 바꾼다.
pub fn char_col(line: &str, utf16: u32) -> usize {
    line[..byte_col(line, utf16)].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_columns_are_identity() {
        assert_eq!(utf16_col("hello", 3), 3);
        assert_eq!(byte_col("hello", 3), 3);
        assert_eq!(char_col("hello", 5), 5);
    }

    #[test]
    fn korean_is_one_unit_but_three_bytes() {
        let s = "a한글b";
        assert_eq!(utf16_col(s, 1), 1);
        assert_eq!(utf16_col(s, 4), 2);
        assert_eq!(utf16_col(s, 7), 3);
        assert_eq!(byte_col(s, 2), 4);
        assert_eq!(byte_col(s, 3), 7);
        assert_eq!(char_col(s, 3), 3);
    }

    #[test]
    fn emoji_uses_surrogate_pair() {
        let s = "x😀y";
        assert_eq!(utf16_col(s, 1), 1);
        assert_eq!(utf16_col(s, 5), 3);
        assert_eq!(byte_col(s, 3), 5);
        assert_eq!(byte_col(s, 2), 1, "서로게이트 쌍 가운데는 문자 앞으로");
        assert_eq!(char_col(s, 3), 2);
        assert_eq!(char_col(s, 4), 3);
    }

    #[test]
    fn out_of_range_and_mid_char_are_clamped() {
        assert_eq!(byte_col("ab", 99), 2);
        assert_eq!(utf16_col("ab", 99), 2);
        assert_eq!(utf16_col("한", 1), 0);
        assert_eq!(byte_col("", 3), 0);
    }
}
