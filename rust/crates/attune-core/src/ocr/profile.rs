//! OCR 场景预设 (OcrProfile) — 用户可见的"场景预设"概念.
//!
//! README.md 「用户可配置项」承诺: 用户看到的是**场景名** (合同 / 票据 / 截图 / 古籍 / 表格…),
//! 不是引擎/模型/DPI 等技术参数. 同时可配置多个 profile.
//!
//! 当前唯一引擎是 PP-OCRv5 mobile, profile 真正能调的参数是 PDF 渲染 DPI
//! (200 普通文档 / 300 标准合同 / 600 古籍高分辨率) 及表格重建开关.

use serde::{Deserialize, Serialize};

/// OCR 场景预设. `builtin = true` 的预设不可删不可改.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OcrProfile {
    /// slug id, e.g. "contract" / "receipt"
    pub id: String,
    /// 显示名, e.g. "合同 / 法律文书"
    pub name: String,
    /// 用户可见说明, 1-2 句
    pub description: String,
    /// 语言代码 (tesseract 风格 "chi_sim+eng"), 当前 PP-OCRv5 模型内置中英,
    /// 此字段作为元信息存留, 供未来扩展 / UI 显示
    pub languages: String,
    /// PDF 渲染 DPI — profile 真正能控制的参数 (200 / 300 / 600)
    pub dpi: u32,
    /// 适用场景标签, e.g. ["合同", "判决书", "起诉状"]
    pub tags: Vec<String>,
    /// 内置预设, 不允许通过 API 删除或修改
    pub builtin: bool,
    /// 是否尝试重建表格结构（输出 Markdown 表格）。
    /// 开启时 OCR pipeline 按文本块坐标分析行列关系，在 `OcrOutput::table_markdown` 填入
    /// `| 列1 | 列2 |` 格式。table / form / receipt 场景默认开启；其他场景默认关闭。
    #[serde(default)]
    pub reconstruct_tables: bool,
    /// 是否在 OCR 前做文档去斜（Deskew）预处理。
    /// 扫描件偏斜 > 0.5° 时自动旋转校正，提升识别精度。
    /// contract / ancient / table 等扫描类场景默认开启；截图类默认关闭。
    #[serde(default)]
    pub deskew: bool,
    /// OCR 推理时长边缩放上限（像素）。PP-OCR 把图片长边缩到此值再识别 ——
    /// 越大越清晰、越慢。法律证据（合同 / 借条 / 流水）建议 ≥3200 保留小字细节。
    /// 0 或越界值在 OCR 调用处回退到 `DEFAULT_MAX_SIDE_LEN`。
    #[serde(default = "default_max_side_len")]
    pub max_side_len: u32,
}

/// `max_side_len` 的 serde 默认值 —— 旧 profile JSON 缺该字段时回退。
fn default_max_side_len() -> u32 {
    OcrProfile::DEFAULT_MAX_SIDE_LEN
}

impl OcrProfile {
    /// 返回 7 个内置场景预设 (合同 / 票据 / 截图 / 古籍 / 表格 / 表单 / 证件).
    pub fn builtins() -> Vec<OcrProfile> {
        vec![
            OcrProfile {
                id: "contract".to_string(),
                name: "合同 / 法律文书".to_string(),
                description: "适合扫描合同、判决书、起诉状等结构化法律文档".to_string(),
                languages: "chi_sim+eng".to_string(),
                dpi: 300,
                tags: vec!["合同".to_string(), "判决书".to_string(), "起诉状".to_string()],
                builtin: true,
                reconstruct_tables: false,
                deskew: true,
                max_side_len: 3200,
            },
            OcrProfile {
                id: "receipt".to_string(),
                name: "票据 / 流水".to_string(),
                description: "适合发票、银行流水、收据等小尺寸票据".to_string(),
                languages: "chi_sim+eng".to_string(),
                dpi: 300,
                tags: vec!["票据".to_string(), "发票".to_string(), "流水".to_string()],
                builtin: true,
                reconstruct_tables: true,
                deskew: false,
                max_side_len: 3200,
            },
            OcrProfile {
                id: "screenshot".to_string(),
                name: "屏幕截图".to_string(),
                description: "适合聊天截图、网页截图等屏幕原始分辨率图片".to_string(),
                languages: "chi_sim+eng".to_string(),
                dpi: 200,
                tags: vec!["聊天截图".to_string(), "网页截图".to_string()],
                builtin: true,
                reconstruct_tables: false,
                deskew: false,
                max_side_len: 2048,
            },
            OcrProfile {
                id: "ancient".to_string(),
                name: "古籍 / 碑文".to_string(),
                description: "适合古籍扫描件、碑文拓片等高分辨率纯中文场景".to_string(),
                languages: "chi_sim".to_string(),
                dpi: 600,
                tags: vec!["古籍".to_string(), "碑文".to_string()],
                builtin: true,
                reconstruct_tables: false,
                deskew: true,
                max_side_len: 4096,
            },
            OcrProfile {
                id: "table".to_string(),
                name: "表格 / 报表".to_string(),
                description: "适合 Excel 导出表格、财务报表、数据汇总等规整表格文档，输出 Markdown 表格格式".to_string(),
                languages: "chi_sim+eng".to_string(),
                dpi: 300,
                tags: vec!["表格".to_string(), "报表".to_string(), "数据".to_string(), "财务".to_string()],
                builtin: true,
                reconstruct_tables: true,
                deskew: true,
                max_side_len: 3200,
            },
            OcrProfile {
                id: "form".to_string(),
                name: "表单 / 填写项".to_string(),
                description: "适合申请表、问卷、登记表等有填写字段的表单，识别字段名与填写内容".to_string(),
                languages: "chi_sim+eng".to_string(),
                dpi: 200,
                tags: vec!["表单".to_string(), "申请表".to_string(), "问卷".to_string()],
                builtin: true,
                reconstruct_tables: true,
                deskew: false,
                max_side_len: 2560,
            },
            OcrProfile {
                id: "card".to_string(),
                name: "证件 / 名片".to_string(),
                description: "适合身份证、营业执照、名片等小尺寸证件类图片".to_string(),
                languages: "chi_sim+eng".to_string(),
                dpi: 300,
                tags: vec!["身份证".to_string(), "营业执照".to_string(), "名片".to_string()],
                builtin: true,
                reconstruct_tables: false,
                deskew: false,
                max_side_len: 2560,
            },
        ]
    }

    /// 默认 profile id (用户没显式指定 profile 时用)
    pub const DEFAULT_ID: &'static str = "contract";

    /// OCR 长边缩放上限默认值 —— profile 未指定 / 越界时回退。
    pub const DEFAULT_MAX_SIDE_LEN: u32 = 2048;

    /// 校验 dpi 合法 (PP-OCR 在 200-600 之间最稳定)
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("id 不能为空".to_string());
        }
        if !self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err("id 只允许 [a-zA-Z0-9_-]".to_string());
        }
        if self.name.trim().is_empty() {
            return Err("name 不能为空".to_string());
        }
        if !(72..=1200).contains(&self.dpi) {
            return Err(format!("dpi {} 超出 [72, 1200] 范围", self.dpi));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_count_is_seven() {
        let bs = OcrProfile::builtins();
        assert_eq!(bs.len(), 7);
        let ids: Vec<&str> = bs.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, vec!["contract", "receipt", "screenshot", "ancient", "table", "form", "card"]);
    }

    #[test]
    fn builtins_all_marked_builtin() {
        for p in OcrProfile::builtins() {
            assert!(p.builtin, "{} should be builtin", p.id);
        }
    }

    #[test]
    fn default_id_exists_in_builtins() {
        let ids: Vec<String> = OcrProfile::builtins().into_iter().map(|p| p.id).collect();
        assert!(ids.iter().any(|i| i == OcrProfile::DEFAULT_ID));
    }

    #[test]
    fn validate_rejects_empty_id() {
        let mut p = OcrProfile::builtins()[0].clone();
        p.id.clear();
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_bad_id_chars() {
        let mut p = OcrProfile::builtins()[0].clone();
        p.id = "with space".to_string();
        assert!(p.validate().is_err());
        p.id = "with/slash".to_string();
        assert!(p.validate().is_err());
    }

    #[test]
    fn validate_rejects_extreme_dpi() {
        let mut p = OcrProfile::builtins()[0].clone();
        p.dpi = 50;
        assert!(p.validate().is_err());
        p.dpi = 2000;
        assert!(p.validate().is_err());
        p.dpi = 300;
        assert!(p.validate().is_ok());
    }

    #[test]
    fn serde_roundtrip() {
        let p = OcrProfile::builtins()[0].clone();
        let j = serde_json::to_string(&p).expect("ser");
        let back: OcrProfile = serde_json::from_str(&j).expect("de");
        assert_eq!(p, back);
    }

    #[test]
    fn reconstruct_tables_flag_on_table_profile() {
        let bs = OcrProfile::builtins();
        let table = bs.iter().find(|p| p.id == "table").unwrap();
        assert!(table.reconstruct_tables, "table profile must have reconstruct_tables=true");
        let form = bs.iter().find(|p| p.id == "form").unwrap();
        assert!(form.reconstruct_tables, "form profile must have reconstruct_tables=true");
        let contract = bs.iter().find(|p| p.id == "contract").unwrap();
        assert!(!contract.reconstruct_tables, "contract profile should not force table reconstruction");
    }

    #[test]
    fn deskew_flag_on_scan_profiles() {
        let bs = OcrProfile::builtins();
        let contract = bs.iter().find(|p| p.id == "contract").unwrap();
        assert!(contract.deskew, "contract (scanned) should deskew");
        let screenshot = bs.iter().find(|p| p.id == "screenshot").unwrap();
        assert!(!screenshot.deskew, "screenshot should not deskew");
    }

    #[test]
    fn serde_roundtrip_new_fields_default_false_on_missing() {
        // Legacy JSON without reconstruct_tables / deskew fields should deserialize with false
        let legacy_json = r#"{
            "id":"contract","name":"合同","description":"desc",
            "languages":"chi_sim","dpi":300,"tags":[],"builtin":true
        }"#;
        let p: OcrProfile = serde_json::from_str(legacy_json).expect("deserialize legacy");
        assert!(!p.reconstruct_tables);
        assert!(!p.deskew);
    }
}
