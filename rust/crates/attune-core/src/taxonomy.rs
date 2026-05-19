use crate::error::{Result, VaultError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum Cardinality {
    Single,
    Multi { max: usize },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ValueType {
    Open,
    Closed,
    Hybrid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Dimension {
    pub name: String,
    pub label: String,
    pub description: String,
    pub cardinality: Cardinality,
    pub value_type: ValueType,
    #[serde(default)]
    pub suggested_values: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Plugin {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub dimensions: Vec<Dimension>,
    #[serde(default)]
    pub prompt_hint: String,
}

impl Plugin {
    pub fn from_yaml(yaml: &str) -> Result<Self> {
        serde_yaml::from_str(yaml).map_err(VaultError::from)
    }
}

// v0.6 OSS 边界瘦身（per docs/oss-pro-strategy.md v2）：
// 行业 builtin plugins (law / presales / patent / tech) 全部迁移到 attune-pro 仓的
// plugins/<vertical>-pro/builtin/dimensions.yaml。OSS attune 不再内置任何行业分类维度。
//
// 加载顺序：attune-server 启动 → load_builtin_plugins() 返回空 Vec
// → 用户安装 vertical plugin pack (attune-pro/.attunepkg) 后从 plugin_registry 动态加载
// 详见 attune-pro/INTEGRATION.md §13 OSS 委托给 Pro 的 vertical-specific functionality。

pub struct Taxonomy {
    pub core: Vec<Dimension>,
    pub universal: Vec<Dimension>,
    pub plugins: Vec<Plugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub version: u32,
    pub classified_at: String,
    pub model: String,
    pub plugins_used: Vec<String>,
    pub core: HashMap<String, Vec<String>>,
    pub universal: HashMap<String, String>,
    pub plugin: HashMap<String, HashMap<String, Vec<String>>>,
    #[serde(default)]
    pub user_tags: Vec<String>,
}

impl ClassificationResult {
    pub fn empty() -> Self {
        Self {
            version: 1,
            classified_at: chrono::Utc::now().to_rfc3339(),
            model: String::new(),
            plugins_used: vec![],
            core: HashMap::new(),
            universal: HashMap::new(),
            plugin: HashMap::new(),
            user_tags: vec![],
        }
    }
}

impl Default for Taxonomy {
    fn default() -> Self {
        Self {
            core: Self::build_core_dimensions(),
            universal: Self::build_universal_dimensions(),
            plugins: vec![],
        }
    }
}

impl Taxonomy {
    /// v0.6 OSS 瘦身：返回空列表（行业 builtin 全部迁移到 attune-pro）。
    /// 保留 fn 签名兼容现有 attune-server::state.rs 调用。
    pub fn load_builtin_plugins() -> Result<Vec<Plugin>> {
        Ok(Vec::new())
    }

    /// v0.6 OSS 瘦身：所有 id 都返 unknown（无 builtin 行业 plugin）。
    /// 行业插件由 attune-pro/.attunepkg 安装后通过 PluginRegistry::scan 动态加载。
    pub fn load_builtin_plugin(id: &str) -> Result<Plugin> {
        Err(VaultError::Taxonomy(format!(
            "no builtin plugin '{id}': install attune-pro vertical plugin pack instead"
        )))
    }

    /// 从 {config_dir}/plugins/*.yaml 加载用户插件
    /// 返回 (成功加载的插件列表, 失败的文件名和错误)
    pub fn load_user_plugins(config_dir: &std::path::Path) -> (Vec<Plugin>, Vec<(String, String)>) {
        let plugins_dir = config_dir.join("plugins");
        let mut loaded = Vec::new();
        let mut errors = Vec::new();

        if !plugins_dir.exists() {
            return (loaded, errors);
        }

        let entries = match std::fs::read_dir(&plugins_dir) {
            Ok(e) => e,
            Err(_) => return (loaded, errors),
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let ext = path.extension().and_then(|e| e.to_str());
            if ext != Some("yaml") && ext != Some("yml") {
                continue;
            }
            let filename = path.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("unknown")
                .to_string();

            match std::fs::read_to_string(&path) {
                Ok(content) => match Plugin::from_yaml(&content) {
                    Ok(plugin) => loaded.push(plugin),
                    Err(e) => errors.push((filename, e.to_string())),
                },
                Err(e) => errors.push((filename, format!("read error: {e}"))),
            }
        }

        (loaded, errors)
    }

    pub fn with_plugin(mut self, plugin: Plugin) -> Self {
        self.plugins.push(plugin);
        self
    }

    pub fn build_system_prompt(&self) -> String {
        let mut s = String::from("你是一个知识库自动分类助手。给定文本内容，输出严格的 JSON 分类结果。\n\n");
        s.push_str("维度定义:\n\n");

        s.push_str("## 核心维度 (core):\n");
        for d in &self.core {
            s.push_str(&format_dimension(d));
        }

        s.push_str("\n## 通用扩展维度 (universal):\n");
        for d in &self.universal {
            s.push_str(&format_dimension(d));
        }

        if !self.plugins.is_empty() {
            s.push_str("\n## 插件维度 (plugin):\n");
            for p in &self.plugins {
                s.push_str(&format!("\n### 插件 {} ({})\n{}\n", p.id, p.name, p.prompt_hint));
                for d in &p.dimensions {
                    s.push_str(&format_dimension(d));
                }
            }
        }

        s.push_str("\n## 输出格式 (严格遵守):\n");
        s.push_str("{\n  \"core\": {\"domain\": [...], \"topic\": [...], \"purpose\": [...], \"project\": [...], \"entities\": [...]},\n");
        s.push_str("  \"universal\": {\"difficulty\": \"...\", \"freshness\": \"...\", \"action_type\": \"...\"},\n");
        s.push_str("  \"plugin\": {");
        for (i, p) in self.plugins.iter().enumerate() {
            if i > 0 { s.push_str(", "); }
            let dims: Vec<String> = p.dimensions.iter().map(|d| format!("\"{}\": [...]", d.name)).collect();
            s.push_str(&format!("\"{}\": {{{}}}", p.id, dims.join(", ")));
        }
        s.push_str("}\n}\n\n");
        s.push_str("规则:\n- 数组字段至少 1 个值\n- Closed 类型只能从候选值中选择\n- Hybrid 类型优先从候选值选择\n- 批量输入时返回 JSON 数组，顺序与输入一致\n");
        s
    }

    pub fn build_user_prompt(&self, items: &[(String, String)]) -> String {
        let mut s = format!("请分类以下 {} 条内容:\n\n", items.len());
        for (i, (title, content)) in items.iter().enumerate() {
            let truncated: String = content.chars().take(2000).collect();
            s.push_str(&format!("[{}]\n标题: {}\n内容: {}\n\n", i + 1, title, truncated));
        }
        if items.len() == 1 {
            s.push_str("输出 JSON 对象（非数组）。\n");
        } else {
            s.push_str(&format!("输出 JSON 数组，包含 {} 个对象，顺序对应。\n", items.len()));
        }
        s
    }

    pub fn validate(&self, result: &ClassificationResult) -> Result<()> {
        for d in &self.core {
            if !result.core.contains_key(&d.name) {
                return Err(VaultError::Classification(format!("missing core dimension: {}", d.name)));
            }
            let values = &result.core[&d.name];
            self.check_cardinality(&d.cardinality, values.len(), &d.name)?;
            self.check_value_type(&d.value_type, &d.suggested_values, values, &d.name)?;
        }
        for d in &self.universal {
            if !result.universal.contains_key(&d.name) {
                return Err(VaultError::Classification(format!("missing universal dimension: {}", d.name)));
            }
            let value = &result.universal[&d.name];
            self.check_value_type(&d.value_type, &d.suggested_values, std::slice::from_ref(value), &d.name)?;
        }
        Ok(())
    }

    fn check_cardinality(&self, c: &Cardinality, count: usize, name: &str) -> Result<()> {
        match c {
            Cardinality::Single if count != 1 => {
                Err(VaultError::Classification(format!("dimension {name} expects single value, got {count}")))
            }
            Cardinality::Multi { max } if count > *max || count == 0 => {
                Err(VaultError::Classification(format!("dimension {name} expects 1..={max} values, got {count}")))
            }
            _ => Ok(()),
        }
    }

    fn check_value_type(&self, vt: &ValueType, allowed: &[String], values: &[String], name: &str) -> Result<()> {
        if matches!(vt, ValueType::Closed) {
            for v in values {
                if !allowed.iter().any(|a| a == v) {
                    return Err(VaultError::Classification(format!("dimension {name} closed value {v} not in allowed set")));
                }
            }
        }
        Ok(())
    }

    fn build_core_dimensions() -> Vec<Dimension> {
        vec![
            Dimension {
                name: "domain".into(),
                label: "领域".into(),
                description: "所属行业或专业领域".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Hybrid,
                suggested_values: vec![
                    "技术".into(), "商业".into(), "法律".into(), "医疗".into(),
                    "金融".into(), "生活".into(), "学习".into(), "科研".into(),
                    "艺术".into(), "政策".into(),
                ],
            },
            Dimension {
                name: "topic".into(),
                label: "主题".into(),
                description: "具体话题，最多 3 个".into(),
                cardinality: Cardinality::Multi { max: 3 },
                value_type: ValueType::Open,
                suggested_values: vec![],
            },
            Dimension {
                name: "purpose".into(),
                label: "用途".into(),
                description: "知识的角色定位".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "参考资料".into(), "个人笔记".into(), "待办草稿".into(),
                    "问答记录".into(), "归档".into(), "灵感".into(),
                ],
            },
            Dimension {
                name: "project".into(),
                label: "项目".into(),
                description: "所属项目或上下文".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Open,
                suggested_values: vec![],
            },
            Dimension {
                name: "entities".into(),
                label: "实体".into(),
                description: "涉及的人物、组织、产品等命名实体".into(),
                cardinality: Cardinality::Multi { max: 10 },
                value_type: ValueType::Open,
                suggested_values: vec![],
            },
        ]
    }

    fn build_universal_dimensions() -> Vec<Dimension> {
        vec![
            Dimension {
                name: "difficulty".into(),
                label: "深度".into(),
                description: "内容的专业深度".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "入门".into(), "进阶".into(), "专家".into(), "N/A".into(),
                ],
            },
            Dimension {
                name: "freshness".into(),
                label: "时效".into(),
                description: "知识的保质期".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "常青".into(), "时效性".into(), "已过期".into(),
                ],
            },
            Dimension {
                name: "action_type".into(),
                label: "行动".into(),
                description: "是否需要采取行动".into(),
                cardinality: Cardinality::Single,
                value_type: ValueType::Closed,
                suggested_values: vec![
                    "待办".into(), "学习".into(), "参考".into(),
                    "决策依据".into(), "纯归档".into(),
                ],
            },
        ]
    }
}

fn format_dimension(d: &Dimension) -> String {
    let vt_desc = match &d.value_type {
        ValueType::Open => "开放式".to_string(),
        ValueType::Closed => format!("封闭集合 [{}]", d.suggested_values.join(", ")),
        ValueType::Hybrid => format!("混合式 (候选: [{}])", d.suggested_values.join(", ")),
    };
    let card = match &d.cardinality {
        Cardinality::Single => "单值".to_string(),
        Cardinality::Multi { max } => format!("最多 {} 值", max),
    };
    format!("- {} ({}): {} / {} / {}\n", d.name, d.label, d.description, card, vt_desc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_taxonomy_has_core_and_universal() {
        let t = Taxonomy::default();
        assert_eq!(t.core.len(), 5);
        assert_eq!(t.universal.len(), 3);
        assert_eq!(t.plugins.len(), 0);
        let names: Vec<&str> = t.core.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"domain"));
        assert!(names.contains(&"topic"));
        assert!(names.contains(&"purpose"));
        assert!(names.contains(&"project"));
        assert!(names.contains(&"entities"));
    }

    #[test]
    fn load_builtin_plugins_returns_empty() {
        // v0.6 OSS 瘦身：行业 builtin 全部迁到 attune-pro
        let plugins = Taxonomy::load_builtin_plugins().unwrap();
        assert!(plugins.is_empty(), "OSS 不应内置任何行业 plugin");
    }

    #[test]
    fn load_builtin_plugin_unknown_for_all_ids() {
        // 所有原 builtin id 都不再可用
        for id in ["tech", "law", "presales", "patent", "anything"] {
            assert!(
                Taxonomy::load_builtin_plugin(id).is_err(),
                "load_builtin_plugin('{id}') 应返 unknown"
            );
        }
    }

    #[test]
    fn build_system_prompt_with_user_plugin() {
        // 用临时 user plugin 测 build_system_prompt 仍 work（不再依赖 builtin tech）
        let custom = Plugin::from_yaml(
            "id: demo\nname: demo\nversion: \"1.0\"\ndescription: demo\ndimensions:\n  - name: foo\n    label: Foo\n    description: foo dim\n    cardinality:\n      type: Single\n    value_type:\n      type: Open\n    suggested_values: []\n",
        )
        .unwrap();
        let t = Taxonomy::default().with_plugin(custom);
        let prompt = t.build_system_prompt();
        assert!(prompt.contains("domain"));
        assert!(prompt.contains("topic"));
        assert!(prompt.contains("difficulty"));
        assert!(prompt.contains("foo"));
        assert!(prompt.contains("JSON"));
    }

    #[test]
    fn build_user_prompt_batch() {
        let t = Taxonomy::default();
        let items = vec![
            ("Title A".to_string(), "Content A".to_string()),
            ("Title B".to_string(), "Content B".to_string()),
        ];
        let prompt = t.build_user_prompt(&items);
        assert!(prompt.contains("[1]"));
        assert!(prompt.contains("[2]"));
        assert!(prompt.contains("Title A"));
        assert!(prompt.contains("Title B"));
        assert!(prompt.contains("JSON 数组"));
    }

    #[test]
    fn load_user_plugins_empty_dir() {
        let tmp = tempfile::TempDir::new().unwrap();
        let (loaded, errors) = Taxonomy::load_user_plugins(tmp.path());
        assert!(loaded.is_empty());
        assert!(errors.is_empty());
    }

    #[test]
    fn load_user_plugins_parses_yaml() {
        let tmp = tempfile::TempDir::new().unwrap();
        let plugins_dir = tmp.path().join("plugins");
        std::fs::create_dir_all(&plugins_dir).unwrap();

        let yaml = r#"
id: finance
name: 金融
version: "1.0"
description: 金融相关
dimensions:
  - name: asset_class
    label: 资产类别
    description: 资产的类型
    cardinality:
      type: Single
    value_type:
      type: Hybrid
    suggested_values:
      - 股票
      - 债券
      - 基金
prompt_hint: |
  这是金融内容
"#;
        std::fs::write(plugins_dir.join("finance.yaml"), yaml).unwrap();

        let (loaded, errors) = Taxonomy::load_user_plugins(tmp.path());
        assert_eq!(loaded.len(), 1);
        assert!(errors.is_empty());
        assert_eq!(loaded[0].id, "finance");
        assert_eq!(loaded[0].dimensions.len(), 1);
    }
}
