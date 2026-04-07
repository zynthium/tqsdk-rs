use serde::{Deserialize, Serialize};

/// Range 用来表示一个数据段，包含 start, end 之间的 int64 类型连续整数集合，包含 end - start 个数据点 (左闭开区间)
/// 一个 Range 用元组来表示，例如: (0, 100), (8000, 10000)
/// 在 Rust 中，我们使用结构体来表示 Range，提高可读性。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Range {
    pub start: i64,
    pub end: i64,
}

impl Range {
    pub fn new(start: i64, end: i64) -> Self {
        assert!(start <= end, "Range start must be less than or equal to end");
        Range { start, end }
    }

    /// 检查 Range 是否为空
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// 获取 Range 的长度
    pub fn len(&self) -> i64 {
        self.end - self.start
    }
}

/// RangeSet 是一组有序的、不重叠的、递增的 Range
pub type RangeSet = Vec<Range>;

/// 合并 RangeSet 中所有重叠或相邻的区间
pub fn rangeset_merge(mut ranges: RangeSet) -> RangeSet {
    if ranges.is_empty() {
        return Vec::new();
    }

    // 确保区间是按起始点排序的
    ranges.sort_by_key(|r| r.start);

    let mut merged_ranges = Vec::new();
    let mut current_range = ranges[0].clone();

    for next_range in ranges.iter().skip(1) {
        // 如果当前区间与下一个区间重叠或相邻
        if current_range.end >= next_range.start {
            current_range.end = current_range.end.max(next_range.end);
        } else {
            // 没有重叠或相邻，将当前区间添加到结果中，并开始处理下一个区间
            merged_ranges.push(current_range);
            current_range = next_range.clone();
        }
    }
    merged_ranges.push(current_range); // 添加最后一个区间

    merged_ranges
}

/// 计算两个 RangeSet 的并集
pub fn rangeset_union(a: &RangeSet, b: &RangeSet) -> RangeSet {
    let mut result = Vec::new();
    let mut i = 0;
    let mut j = 0;

    while i < a.len() || j < b.len() {
        let mut current_range = if i < a.len() && (j == b.len() || a[i].start < b[j].start) {
            a[i].clone()
        } else if j < b.len() {
            b[j].clone()
        } else {
            break; // Both rangesets are exhausted
        };

        // Move pointers for the selected initial range
        if i < a.len() && current_range == a[i] {
            i += 1;
        } else if j < b.len() && current_range == b[j] {
            j += 1;
        }

        // Merge with overlapping or adjacent ranges from both sets
        while i < a.len() && current_range.end >= a[i].start {
            current_range.end = current_range.end.max(a[i].end);
            i += 1;
        }
        while j < b.len() && current_range.end >= b[j].start {
            current_range.end = current_range.end.max(b[j].end);
            j += 1;
        }
        result.push(current_range);
    }
    rangeset_merge(result) // 确保最终结果是合并过的
}

/// 计算两个 RangeSet 的交集
pub fn rangeset_intersection(a: &RangeSet, b: &RangeSet) -> RangeSet {
    let mut result = Vec::new();
    let mut i = 0;
    let mut j = 0;

    while i < a.len() && j < b.len() {
        let range_a = &a[i];
        let range_b = &b[j];

        // 找出两个区间的重叠部分
        let start = range_a.start.max(range_b.start);
        let end = range_a.end.min(range_b.end);

        if start < end {
            result.push(Range::new(start, end));
        }

        // 移动指针，总是移动结束时间较早的那个区间的指针
        if range_a.end < range_b.end {
            i += 1;
        } else {
            j += 1;
        }
    }
    result // 交集结果本身就是有序且不重叠的，无需再次合并
}

/// 计算 RangeSet a 与 b 的差集 (a - b)
pub fn rangeset_difference(a: &RangeSet, b: &RangeSet) -> RangeSet {
    let mut result = Vec::new();
    let mut i = 0; // Pointer for RangeSet a
    let mut j = 0; // Pointer for RangeSet b

    while i < a.len() {
        let mut current_a = a[i].clone();

        // 寻找第一个与 current_a 重叠或在其之后的 b[j]
        while j < b.len() && b[j].end <= current_a.start {
            j += 1;
        }

        // 遍历所有与 current_a 重叠的 b[j]
        let mut temp_j = j; // 使用临时指针，不影响外层 j
        while temp_j < b.len() && b[temp_j].start < current_a.end {
            if current_a.start < b[temp_j].start {
                result.push(Range::new(current_a.start, b[temp_j].start));
            }
            current_a.start = current_a.start.max(b[temp_j].end);
            if current_a.start >= current_a.end {
                break;
            }
            temp_j += 1;
        }

        if current_a.start < current_a.end {
            result.push(current_a);
        }
        i += 1;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rangeset_merge_empty() {
        let ranges = vec![];
        let merged = rangeset_merge(ranges);
        assert!(merged.is_empty());
    }

    #[test]
    fn test_rangeset_merge_single_range() {
        let ranges = vec![Range::new(0, 10)];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(0, 10)]);
    }

    #[test]
    fn test_rangeset_merge_no_overlap() {
        let ranges = vec![Range::new(0, 10), Range::new(20, 30)];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(0, 10), Range::new(20, 30)]);
    }

    #[test]
    fn test_rangeset_merge_overlap() {
        let ranges = vec![Range::new(0, 10), Range::new(5, 15)];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(0, 15)]);
    }

    #[test]
    fn test_rangeset_merge_adjacent() {
        let ranges = vec![Range::new(0, 10), Range::new(10, 20)];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(0, 20)]);
    }

    #[test]
    fn test_rangeset_merge_multiple_overlaps() {
        let ranges = vec![
            Range::new(0, 5),
            Range::new(3, 8),
            Range::new(10, 15),
            Range::new(12, 18),
            Range::new(20, 25),
        ];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(0, 8), Range::new(10, 18), Range::new(20, 25)]);
    }

    #[test]
    fn test_rangeset_merge_unsorted_input() {
        let ranges = vec![Range::new(10, 20), Range::new(0, 5), Range::new(3, 8), Range::new(2, 4)];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(0, 8), Range::new(10, 20)]);
    }

    #[test]
    fn test_rangeset_merge_complex() {
        let ranges = vec![
            Range::new(1, 3),
            Range::new(2, 4),
            Range::new(5, 7),
            Range::new(6, 8),
            Range::new(9, 10),
            Range::new(10, 12),
        ];
        let merged = rangeset_merge(ranges);
        assert_eq!(merged, vec![Range::new(1, 4), Range::new(5, 8), Range::new(9, 12)]);
    }

    #[test]
    fn test_rangeset_union_empty() {
        let a = vec![];
        let b = vec![];
        let union = rangeset_union(&a, &b);
        assert!(union.is_empty());
    }

    #[test]
    fn test_rangeset_union_one_empty() {
        let a = vec![Range::new(0, 10)];
        let b = vec![];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 10)]);

        let a = vec![];
        let b = vec![Range::new(0, 10)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 10)]);
    }

    #[test]
    fn test_rangeset_union_no_overlap() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(20, 30)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 10), Range::new(20, 30)]);
    }

    #[test]
    fn test_rangeset_union_overlap() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(5, 15)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 15)]);
    }

    #[test]
    fn test_rangeset_union_adjacent() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(10, 20)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 20)]);
    }

    #[test]
    fn test_rangeset_union_multiple_ranges() {
        let a = vec![Range::new(0, 5), Range::new(10, 15)];
        let b = vec![Range::new(3, 8), Range::new(12, 18)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 8), Range::new(10, 18)]);
    }

    #[test]
    fn test_rangeset_union_superset() {
        let a = vec![Range::new(0, 20)];
        let b = vec![Range::new(5, 15)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(0, 20)]);
    }

    #[test]
    fn test_rangeset_union_complex() {
        let a = vec![Range::new(1, 3), Range::new(5, 7), Range::new(9, 11)];
        let b = vec![Range::new(2, 4), Range::new(6, 8), Range::new(10, 12)];
        let union = rangeset_union(&a, &b);
        assert_eq!(union, vec![Range::new(1, 4), Range::new(5, 8), Range::new(9, 12)]);
    }

    #[test]
    fn test_rangeset_intersection_empty() {
        let a = vec![];
        let b = vec![Range::new(0, 10)];
        let intersection = rangeset_intersection(&a, &b);
        assert!(intersection.is_empty());

        let a = vec![Range::new(0, 10)];
        let b = vec![];
        let intersection = rangeset_intersection(&a, &b);
        assert!(intersection.is_empty());
    }

    #[test]
    fn test_rangeset_intersection_no_overlap() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(20, 30)];
        let intersection = rangeset_intersection(&a, &b);
        assert!(intersection.is_empty());
    }

    #[test]
    fn test_rangeset_intersection_partial_overlap() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(5, 15)];
        let intersection = rangeset_intersection(&a, &b);
        assert_eq!(intersection, vec![Range::new(5, 10)]);
    }

    #[test]
    fn test_rangeset_intersection_full_overlap() {
        let a = vec![Range::new(0, 20)];
        let b = vec![Range::new(5, 15)];
        let intersection = rangeset_intersection(&a, &b);
        assert_eq!(intersection, vec![Range::new(5, 15)]);
    }

    #[test]
    fn test_rangeset_intersection_multiple_ranges() {
        let a = vec![Range::new(0, 5), Range::new(10, 20), Range::new(25, 30)];
        let b = vec![Range::new(3, 8), Range::new(15, 22), Range::new(28, 35)];
        let intersection = rangeset_intersection(&a, &b);
        assert_eq!(
            intersection,
            vec![Range::new(3, 5), Range::new(15, 20), Range::new(28, 30)]
        );
    }

    #[test]
    fn test_rangeset_difference_empty() {
        let a = vec![Range::new(0, 10)];
        let b = vec![];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(difference, vec![Range::new(0, 10)]);

        let a = vec![];
        let b = vec![Range::new(0, 10)];
        let difference = rangeset_difference(&a, &b);
        assert!(difference.is_empty());
    }

    #[test]
    fn test_rangeset_difference_full_overlap() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(0, 10)];
        let difference = rangeset_difference(&a, &b);
        assert!(difference.is_empty());

        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(-5, 15)]; // b 包含 a
        let difference = rangeset_difference(&a, &b);
        assert!(difference.is_empty());
    }

    #[test]
    fn test_rangeset_difference_no_overlap() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(20, 30)];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(difference, vec![Range::new(0, 10)]);
    }

    #[test]
    fn test_rangeset_difference_partial_overlap_left() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(-5, 5)];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(difference, vec![Range::new(5, 10)]);
    }

    #[test]
    fn test_rangeset_difference_partial_overlap_right() {
        let a = vec![Range::new(0, 10)];
        let b = vec![Range::new(5, 15)];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(difference, vec![Range::new(0, 5)]);
    }

    #[test]
    fn test_rangeset_difference_middle_overlap() {
        let a = vec![Range::new(0, 20)];
        let b = vec![Range::new(5, 15)];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(difference, vec![Range::new(0, 5), Range::new(15, 20)]);
    }

    #[test]
    fn test_rangeset_difference_multiple_subtractions() {
        let a = vec![Range::new(0, 30)];
        let b = vec![Range::new(5, 10), Range::new(15, 20), Range::new(25, 28)];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(
            difference,
            vec![
                Range::new(0, 5),
                Range::new(10, 15),
                Range::new(20, 25),
                Range::new(28, 30)
            ]
        );
    }

    #[test]
    fn test_rangeset_difference_complex() {
        let a = vec![Range::new(0, 10), Range::new(20, 30)];
        let b = vec![Range::new(5, 25)];
        let difference = rangeset_difference(&a, &b);
        assert_eq!(difference, vec![Range::new(0, 5), Range::new(25, 30)]);
    }
}
