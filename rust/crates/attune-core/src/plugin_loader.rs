// 插件加载器 —— 统一的 plugin.yaml + prompt.md 结构
//
// ## 设计目标
//
// 对齐 lawcontrol 的插件格式（见 `/data/company/project/lawcontrol/plugins/skills/*/plugin.yaml`），
// 让未来：
//   1. 商业插件（律师 / 售前 / 医疗）通过 PluginHub 分发，一套 YAML 两边都能装
//   2. AI 批注 4 个角度（内置）从 plugin.yaml 加载，改 prompt 不用重新编译
//   3. 用户 / 社区可写自定义插件，放到 `~/.local/share/attune/plugins/` 即生效
//
// ## Plugin 格式
//
// ```yaml
// id: ai_annotation_risk          # 唯一标识
// name: AI 风险批注                # 人类可读名
// type: annotation_angle           # 插件类型（路由到对应 loader）
// version: "1.0.0"
// author: attune-team
// category: general
// description: ...
//
// # 类型专属字段（type=annotation_angle 时）
// label_prefix: "⚠️ 风险"
// default_color: red
//
// constraints:
//   max_findings: 5
//   max_snippet_chars: 150
//   min_snippet_chars: 4
//   temperature: 0.3
//
// prompt_file: prompt.md           # 相对 plugin dir
//
// # 可选：output JSON schema
// output:
//   schema: { ... }
// ```
//
// ## 签名
//
// 见 `plugin_sig.rs`。内置插件走 `Trust::Official`（内嵌公钥）或 `Trust::Unsigned`
// （开发期放行）；外部插件未来 strict 模式强制验签。

use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};

/// 插件清单（从 plugin.yaml 解析）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub plugin_type: String,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub description: String,

    // annotation_angle 专属
    #[serde(default)]
    pub label_prefix: String,
    #[serde(default)]
    pub default_color: String,

    #[serde(default)]
    pub constraints: PluginConstraints,

    #[serde(default)]
    pub prompt_file: Option<String>,

    #[serde(default)]
    pub output: Option<PluginOutputSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginConstraints {
    #[serde(default)]
    pub max_findings: Option<usize>,
    #[serde(default)]
    pub max_snippet_chars: Option<usize>,
    #[serde(default)]
    pub min_snippet_chars: Option<usize>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub output_format: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginOutputSpec {
    #[serde(default)]
    pub schema: Option<serde_json::Value>,
    #[serde(default)]
    pub schema_ref: Option<String>,
}

/// 加载后的完整插件（清单 + prompt 文本）
#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub manifest: PluginManifest,
    pub prompt: String,
}

impl LoadedPlugin {
    /// 从 YAML 字符串 + prompt 字符串构造（内置插件走这条路径）
    pub fn from_strings(yaml: &str, prompt: &str) -> Result<Self> {
        let manifest: PluginManifest = serde_yaml::from_str(yaml)
            .map_err(|e| VaultError::InvalidInput(format!("plugin yaml parse: {e}")))?;
        Ok(Self { manifest, prompt: prompt.to_string() })
    }

    /// 从文件系统路径加载（外部插件走这条路径）。
    /// 调用方应在加载前先跑 `plugin_sig::verify_loose` 决定是否允许加载。
    pub fn from_dir(plugin_dir: &std::path::Path) -> Result<Self> {
        let yaml_bytes = std::fs::read(plugin_dir.join("plugin.yaml"))
            .map_err(VaultError::Io)?;
        let yaml = String::from_utf8(yaml_bytes)
            .map_err(|e| VaultError::InvalidInput(format!("plugin.yaml not utf-8: {e}")))?;
        let manifest: PluginManifest = serde_yaml::from_str(&yaml)
            .map_err(|e| VaultError::InvalidInput(format!("plugin yaml parse: {e}")))?;
        let prompt = if let Some(ref pf) = manifest.prompt_file {
            std::fs::read_to_string(plugin_dir.join(pf)).map_err(VaultError::Io)?
        } else {
            String::new()
        };
        Ok(Self { manifest, prompt })
    }
}

/// AI 批注角度专属配置（从 manifest 的 annotation_angle 字段组装）
#[derive(Debug, Clone)]
pub struct AnnotationAngleConfig {
    pub id: String,
    pub label_prefix: String,
    pub default_color: String,
    pub max_findings: usize,
    pub max_snippet_chars: usize,
    pub min_snippet_chars: usize,
    pub prompt: String,
    pub output_schema: Option<serde_json::Value>,
}

impl AnnotationAngleConfig {
    /// 从 LoadedPlugin 提取 AI 批注角度配置。type 必须是 `annotation_angle`。
    pub fn from_loaded(p: &LoadedPlugin) -> Result<Self> {
        if p.manifest.plugin_type != "annotation_angle" {
            return Err(VaultError::InvalidInput(format!(
                "expected plugin type 'annotation_angle', got '{}'",
                p.manifest.plugin_type
            )));
        }
        if p.manifest.label_prefix.is_empty() {
            return Err(VaultError::InvalidInput(
                "annotation_angle plugin requires non-empty label_prefix".into()
            ));
        }
        Ok(Self {
            id: p.manifest.id.clone(),
            label_prefix: p.manifest.label_prefix.clone(),
            default_color: if p.manifest.default_color.is_empty() { "yellow".into() }
                           else { p.manifest.default_color.clone() },
            max_findings: p.manifest.constraints.max_findings.unwrap_or(5),
            max_snippet_chars: p.manifest.constraints.max_snippet_chars.unwrap_or(150),
            min_snippet_chars: p.manifest.constraints.min_snippet_chars.unwrap_or(4),
            prompt: p.prompt.clone(),
            output_schema: p.manifest.output.as_ref().and_then(|o| o.schema.clone()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_YAML: &str = r#"
id: test_plugin
name: 测试插件
type: annotation_angle
version: "1.0.0"
author: test
category: general
description: 单元测试样例
label_prefix: "🧪 测试"
default_color: blue
constraints:
  max_findings: 3
  max_snippet_chars: 100
  min_snippet_chars: 5
prompt_file: prompt.md
output:
  schema:
    type: object
    required: [findings]
"#;

    #[test]
    fn load_from_strings_parses_all_fields() {
        let p = LoadedPlugin::from_strings(SAMPLE_YAML, "# prompt content").unwrap();
        assert_eq!(p.manifest.id, "test_plugin");
        assert_eq!(p.manifest.plugin_type, "annotation_angle");
        assert_eq!(p.manifest.label_prefix, "🧪 测试");
        assert_eq!(p.manifest.default_color, "blue");
        assert_eq!(p.manifest.constraints.max_findings, Some(3));
        assert_eq!(p.prompt, "# prompt content");
    }

    #[test]
    fn annotation_angle_config_from_loaded() {
        let p = LoadedPlugin::from_strings(SAMPLE_YAML, "prompt").unwrap();
        let c = AnnotationAngleConfig::from_loaded(&p).unwrap();
        assert_eq!(c.id, "test_plugin");
        assert_eq!(c.label_prefix, "🧪 测试");
        assert_eq!(c.default_color, "blue");
        assert_eq!(c.max_findings, 3);
        assert_eq!(c.max_snippet_chars, 100);
        assert_eq!(c.min_snippet_chars, 5);
    }

    #[test]
    fn wrong_plugin_type_rejected_for_annotation_angle() {
        let yaml = r#"
id: other
name: 非 annotation
type: skill
version: "1.0.0"
"#;
        let p = LoadedPlugin::from_strings(yaml, "").unwrap();
        assert!(AnnotationAngleConfig::from_loaded(&p).is_err());
    }

    #[test]
    fn empty_label_prefix_rejected() {
        let yaml = r#"
id: bad
name: 坏插件
type: annotation_angle
version: "1.0.0"
"#;
        let p = LoadedPlugin::from_strings(yaml, "").unwrap();
        let err = AnnotationAngleConfig::from_loaded(&p).unwrap_err();
        assert!(err.to_string().contains("label_prefix"));
    }

    #[test]
    fn default_color_falls_back_to_yellow() {
        let yaml = r#"
id: test
name: t
type: annotation_angle
version: "1.0.0"
label_prefix: "X"
"#;
        let p = LoadedPlugin::from_strings(yaml, "").unwrap();
        let c = AnnotationAngleConfig::from_loaded(&p).unwrap();
        assert_eq!(c.default_color, "yellow");
    }

    #[test]
    fn constraints_defaults_when_missing() {
        let yaml = r#"
id: test
name: t
type: annotation_angle
version: "1.0.0"
label_prefix: "X"
"#;
        let p = LoadedPlugin::from_strings(yaml, "").unwrap();
        let c = AnnotationAngleConfig::from_loaded(&p).unwrap();
        assert_eq!(c.max_findings, 5);        // 默认
        assert_eq!(c.max_snippet_chars, 150); // 默认
        assert_eq!(c.min_snippet_chars, 4);   // 默认
    }

    #[test]
    fn output_schema_preserved() {
        let p = LoadedPlugin::from_strings(SAMPLE_YAML, "").unwrap();
        let c = AnnotationAngleConfig::from_loaded(&p).unwrap();
        assert!(c.output_schema.is_some());
    }

    #[test]
    fn parses_workflow_type_manifest() {
        let yaml = r#"
id: law-pro/evidence_chain
name: 跨证据链推理
type: workflow
version: "1.0.0"
author: attune-pro
description: 律师上传新证据时跨证据链联想（行业层）
"#;
        let manifest: PluginManifest = serde_yaml::from_str(yaml).expect("parse workflow manifest");
        assert_eq!(manifest.id, "law-pro/evidence_chain");
        assert_eq!(manifest.plugin_type, "workflow");
        assert_eq!(manifest.version, "1.0.0");
    }

    #[test]
    fn parses_skill_type_manifest() {
        let yaml = r#"
id: law-pro/contract_review
name: 合同风险审查
type: skill
version: "0.1.0"
"#;
        let manifest: PluginManifest = serde_yaml::from_str(yaml).expect("parse skill manifest");
        assert_eq!(manifest.plugin_type, "skill");
    }
}
