use std::collections::BTreeMap;

use crate::organizer::types::{ItemView, NoiseItem};

/// 取维度众数,剔除少数派维度 item 入 noise(防 clusterer 整体 Err)。
/// 返回 (clean_items, noise_items, mismatch_count)。无向量的 item 也入 noise。
pub fn partition_by_majority_dim(items: Vec<ItemView>) -> (Vec<ItemView>, Vec<NoiseItem>, usize) {
    let mut dim_count: BTreeMap<usize, usize> = BTreeMap::new();
    for it in &items {
        if let Some(e) = &it.embedding {
            *dim_count.entry(e.len()).or_default() += 1;
        }
    }
    let majority = dim_count
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(d, _)| d);
    let mut clean = Vec::new();
    let mut noise = Vec::new();
    let mut mismatch = 0usize;
    for it in items {
        match (&it.embedding, majority) {
            (Some(e), Some(maj)) if e.len() == maj => clean.push(it),
            (Some(_), Some(_)) => {
                mismatch += 1;
                noise.push(NoiseItem {
                    item_id: it.item_id,
                    title: it.title,
                });
            }
            // 无向量 item 无法聚类,直接入 noise
            _ => noise.push(NoiseItem {
                item_id: it.item_id,
                title: it.title,
            }),
        }
    }
    (clean, noise, mismatch)
}

/// fallback:按顶层子目录分组,返回 [(dir, item_ids)]。
pub fn fallback_group_by_dir(items: &[ItemView]) -> Vec<(String, Vec<String>)> {
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for it in items {
        groups
            .entry(it.dir.clone())
            .or_default()
            .push(it.item_id.clone());
    }
    groups.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organizer::types::ItemView;
    fn iv(id: &str, dir: &str, emb: Option<Vec<f32>>) -> ItemView {
        ItemView {
            item_id: id.into(),
            title: id.into(),
            content_snippet: "".into(),
            dir: dir.into(),
            embedding: emb,
        }
    }
    #[test]
    fn dim_mismatch_minority_goes_noise() {
        // 多数 2 维,少数 3 维 → 少数派进 noise
        let items = vec![
            iv("a", "", Some(vec![1.0, 0.0])),
            iv("b", "", Some(vec![0.0, 1.0])),
            iv("c", "", Some(vec![1.0, 2.0, 3.0])),
        ];
        let (clean, noise, mismatch) = partition_by_majority_dim(items);
        assert_eq!(mismatch, 1);
        assert_eq!(noise.len(), 1);
        assert_eq!(clean.len(), 2);
    }
    #[test]
    fn fallback_groups_by_subdir_when_too_few() {
        let items = vec![
            iv("a", "/x", None),
            iv("b", "/x", None),
            iv("c", "/y", None),
        ];
        let groups = fallback_group_by_dir(&items);
        assert_eq!(groups.len(), 2); // /x, /y
    }
}
