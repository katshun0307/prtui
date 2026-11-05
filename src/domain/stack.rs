use std::collections::HashMap;

use crate::domain::pr::{Pr, Stack};

/// 更新日時の降順に並べ, stacked PR をまとめ直す. 1 件を差し替えた後にも使う.
pub fn arrange(mut prs: Vec<Pr>) -> Vec<Pr> {
    for p in &mut prs {
        p.stack = None;
    }
    prs.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
    group_stacks(prs)
}

/// stack をまとめずに更新日時の降順に並べる. 他人の PR が混ざる追加のタブで使う
pub fn arrange_flat(mut prs: Vec<Pr>) -> Vec<Pr> {
    prs.sort_by_key(|p| std::cmp::Reverse(p.updated_at));
    prs
}

/// base が一覧内の別 PR の head になっている PR を親の直後に並べ, `stack` に位置を入れる.
/// stack 同士は stack 内で最も新しい更新日時の降順に並べる. 入力は更新日時の降順であること.
/// 親が一覧に無い PR (他人の PR の上に積んだものなど) は stack の根として扱う.
fn group_stacks(prs: Vec<Pr>) -> Vec<Pr> {
    let heads: HashMap<&str, usize> = prs
        .iter()
        .enumerate()
        .map(|(i, p)| (p.head_ref_name.as_str(), i))
        .collect();
    let parent: Vec<Option<usize>> = prs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            heads
                .get(p.base_ref_name.as_str())
                .copied()
                .filter(|&j| j != i)
        })
        .collect();
    // 入力順のまま push するので, 子も更新日時の降順になる
    let mut children = vec![vec![]; prs.len()];
    for (i, p) in parent.iter().enumerate() {
        if let Some(p) = p {
            children[*p].push(i);
        }
    }

    // 各根から深さ優先でたどり (PR の添字, 深さ) を並べる. 循環していれば visited で止まる
    let mut visited = vec![false; prs.len()];
    let mut groups: Vec<Vec<(usize, usize)>> = vec![];
    let roots: Vec<usize> = (0..prs.len()).filter(|&i| parent[i].is_none()).collect();
    for root in roots.into_iter().chain(0..prs.len()) {
        if visited[root] {
            continue;
        }
        let mut group = vec![];
        let mut todo = vec![(root, 0)];
        while let Some((i, depth)) = todo.pop() {
            if std::mem::replace(&mut visited[i], true) {
                continue;
            }
            group.push((i, depth));
            todo.extend(children[i].iter().rev().map(|&c| (c, depth + 1)));
        }
        groups.push(group);
    }
    // 更新日時の降順に入力されているので, 各 stack の最小の添字が最新の更新になる
    groups.sort_by_key(|g| g.iter().map(|&(i, _)| i).min());

    let mut slots: Vec<Option<Pr>> = prs.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(slots.len());
    for group in groups {
        let len = group.iter().map(|&(_, d)| d).max().unwrap_or(0) + 1;
        let stacked = group.len() > 1;
        for (i, depth) in group {
            let mut p = slots[i].take().expect("各 PR は 1 つの stack にだけ入る");
            if stacked {
                p.stack = Some(Stack {
                    pos: depth + 1,
                    len,
                });
            }
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::group_stacks;
    use crate::domain::pr::Pr;

    fn layout(prs: Vec<Pr>) -> Vec<(u64, Option<(usize, usize)>)> {
        group_stacks(prs)
            .into_iter()
            .map(|p| (p.number, p.stack.map(|s| (s.pos, s.len))))
            .collect()
    }

    #[test]
    fn stack_is_grouped_from_bottom() {
        // 更新日時の降順: 3 (最上段) が最新, 間に単独の 9 が挟まる
        let prs = vec![
            Pr::fixture(3, "c", "b"),
            Pr::fixture(9, "x", "main"),
            Pr::fixture(1, "a", "main"),
            Pr::fixture(2, "b", "a"),
        ];
        assert_eq!(
            layout(prs),
            vec![
                (1, Some((1, 3))),
                (2, Some((2, 3))),
                (3, Some((3, 3))),
                (9, None),
            ]
        );
    }

    #[test]
    fn branching_stack_and_missing_parent() {
        let prs = vec![
            Pr::fixture(5, "e", "other"),
            Pr::fixture(2, "b", "a"),
            Pr::fixture(3, "c", "a"),
            Pr::fixture(1, "a", "main"),
        ];
        assert_eq!(
            layout(prs),
            vec![
                (5, None),
                (1, Some((1, 2))),
                (2, Some((2, 2))),
                (3, Some((2, 2))),
            ]
        );
    }

    #[test]
    fn cycle_does_not_hang() {
        let prs = vec![Pr::fixture(1, "a", "b"), Pr::fixture(2, "b", "a")];
        assert_eq!(layout(prs).len(), 2);
    }

    #[test]
    fn arrange_sorts_by_updated_desc_and_resets_stack() {
        use super::arrange;
        use crate::domain::pr::Stack;
        use chrono::{TimeZone, Utc};

        let at = |n: u64, head: &str, base: &str, day: u32| {
            let mut p = Pr::fixture(n, head, base);
            p.updated_at = Utc.with_ymd_and_hms(2026, 1, day, 0, 0, 0).unwrap();
            // 前回の stack が残っていても付け直される
            p.stack = Some(Stack { pos: 9, len: 9 });
            p
        };
        let prs = vec![
            at(1, "a", "main", 1),
            at(2, "x", "main", 3),
            at(3, "y", "main", 2),
        ];
        let got: Vec<_> = arrange(prs)
            .into_iter()
            .map(|p| (p.number, p.stack))
            .collect();
        assert_eq!(got, vec![(2, None), (3, None), (1, None)]);
    }
}
