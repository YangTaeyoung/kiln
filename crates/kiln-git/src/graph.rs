//! 커밋 로그의 간단한 그래프 레인 계산.

use crate::repo::Commit;

/// 한 행의 그래프 정보.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GraphRow {
    /// 이 커밋이 놓인 레인.
    pub col: usize,
    /// 행 위쪽에서 들어오는 선: (시작 레인, 도착 레인). 도착이 `col` 이면 커밋 점으로 모인다.
    pub top: Vec<(usize, usize)>,
    /// 커밋 점에서 행 아래쪽으로 나가는 선의 도착 레인.
    pub bottom_from_commit: Vec<usize>,
    /// 커밋을 거치지 않고 통과하는 선: (위 레인, 아래 레인).
    pub pass: Vec<(usize, usize)>,
    /// 이 행에서 쓰이는 레인 수.
    pub width: usize,
}

/// 커밋 목록(자식 → 부모 순)에 대한 레인을 계산한다.
pub fn compute_graph(commits: &[Commit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for c in commits {
        let before = lanes.clone();
        let col = match lanes.iter().position(|l| l.as_deref() == Some(c.sha.as_str())) {
            Some(i) => i,
            None => match lanes.iter().position(Option::is_none) {
                Some(i) => i,
                None => {
                    lanes.push(None);
                    lanes.len() - 1
                }
            },
        };
        // 이 커밋을 기다리던 다른 레인은 닫는다.
        for l in lanes.iter_mut() {
            if l.as_deref() == Some(c.sha.as_str()) {
                *l = None;
            }
        }
        let mut targets = Vec::new();
        if let Some(p0) = c.parents.first() {
            lanes[col] = Some(p0.clone());
            targets.push(col);
        }
        for p in c.parents.iter().skip(1) {
            if let Some(i) = lanes.iter().position(|l| l.as_deref() == Some(p.as_str())) {
                targets.push(i);
            } else {
                let i = match lanes.iter().position(Option::is_none) {
                    Some(i) => i,
                    None => {
                        lanes.push(None);
                        lanes.len() - 1
                    }
                };
                lanes[i] = Some(p.clone());
                targets.push(i);
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }

        let mut top = Vec::new();
        let mut pass = Vec::new();
        for (i, l) in before.iter().enumerate() {
            let Some(sha) = l else { continue };
            if sha == &c.sha {
                top.push((i, col));
            } else if lanes.get(i).is_some_and(|x| x.as_deref() == Some(sha.as_str())) {
                pass.push((i, i));
            } else if let Some(j) = lanes.iter().position(|x| x.as_deref() == Some(sha.as_str())) {
                pass.push((i, j));
            }
        }
        let width = before.len().max(lanes.len()).max(col + 1);
        rows.push(GraphRow { col, top, bottom_from_commit: targets, pass, width });
    }
    rows
}
