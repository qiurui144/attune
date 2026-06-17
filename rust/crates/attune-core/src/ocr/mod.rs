//! OCR — 单一引擎 PP-OCRv5 mobile (via kreuzberg-paddle-ocr + ort 2.0)
//!
//! 设计选择（2026-05-01）：
//! - **彻底移除 tesseract** — fallback 是幻想保险（网络故障时两个都没救；
//!   ort 兼容性已被 reranker 验证；语言覆盖 PP-OCR 6627 chars 够用）
//! - **不暴露 OCR 引擎开关** — 引擎对用户是实现细节，UI 越少越好
//! - **postinst 自动下载 4 个 ONNX 模型 (~21 MB)** — 装包即可用
//! - PDF → 多页 PNG 经 `pdftoppm` (poppler-utils 自动装)；每页走 PP-OCR
//!
//! 场景自动选择（2026-05-09）：
//! - `auto_detect_scene(path)` 按文件名/内容启发式推荐 profile_id
//! - OcrProfile 新增 `reconstruct_tables` / `deskew` 字段
//! - OcrProvider trait 新增 `extract_structured()` 方法返回 `OcrOutput`
//! - 表格场景输出 Markdown 格式；扫描件场景自动去斜
//!
//! 调用层 (parser.rs / ai_stack.rs) 用：
//!   `detect_default_provider()` → `Box<dyn OcrProvider>`
//!   `extract_text_from_pdf(provider, path)` → 全文
//!   `needs_ocr(text)` → 文字层薄判定（保留旧 API）

pub mod ppocr;
pub mod profile;
pub mod profile_registry;
pub mod structured;

#[cfg(feature = "nontext")]
pub mod nontext;

use crate::error::{Result, VaultError};
use profile::OcrProfile;
use std::path::Path;
use std::process::Command;

// When `nontext` is off, the Region/report types are unavailable; alias to a
// zero-variant placeholder so OcrOutput's Option<...> fields type-check and are
// always `None`. This keeps OcrOutput's shape stable across feature configs.
#[cfg(feature = "nontext")]
use crate::ocr::nontext as nontext_regions;
#[cfg(not(feature = "nontext"))]
mod nontext_regions {
    #[derive(Debug, Clone)]
    pub enum Region {}
    #[derive(Debug, Clone)]
    pub enum OcrCorrectionReport {}
}

/// 单行 OCR 输出（含 bbox 坐标，办公助理结构化抽取需要）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RawLine {
    pub text: String,
    pub bbox: BBox,
    pub confidence: f32,
}

/// 像素坐标 bbox（左上角 + 宽高）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// OCR 输出 — 除纯文本外还携带可选的结构化信息。
#[derive(Debug, Clone)]
pub struct OcrOutput {
    /// 纯文本（始终存在，供全文搜索/摘要）
    pub text: String,
    /// Markdown 表格（仅当 profile.reconstruct_tables=true 且检测到表格布局时填入）
    /// 格式示例：`| 列A | 列B |\n|---|---|\n| v1 | v2 |`
    pub table_markdown: Option<String>,
    /// 文档级 OCR 置信度（0.0-1.0），按文本长度加权平均各文本块的
    /// recognition score。`None` = provider 不提供（非 PP-OCR / 默认实现）。
    /// 下游（grounded 抽取器 / UI）用它判断证据 OCR 是否可信、是否需律师复核。
    pub avg_confidence: Option<f32>,
    /// 行级 OCR 输出（含 bbox），用于 office helper 结构化抽取。
    /// `None` = provider 不支持（默认实现 / mock）；`Some` = PP-OCR 等真实 provider 填充。
    pub lines: Option<Vec<RawLine>>,
    /// Non-text recognition regions (Stage 1-4 output). `None` = pass not run (old behavior).
    pub regions: Option<Vec<nontext_regions::Region>>,
    /// OCR cross-validation / correction report. `None` = cross-validation not run.
    pub correction_report: Option<nontext_regions::OcrCorrectionReport>,
}

/// OCR provider 抽象 — 当前只有 PP-OCRv5 一个实现。
/// trait 仍然保留是为了：测试用 mock + 未来可能的 K3 远程 provider。
pub trait OcrProvider: Send + Sync {
    /// 引擎名（用于日志 / diagnostics 端点）
    fn name(&self) -> &str;

    /// 是否支持中文
    fn has_chinese(&self) -> bool;

    /// 抽取单张图片的文字（plain text，不做表格重建 / 去斜）
    fn extract_text_from_image(&self, image_path: &Path) -> Result<String>;

    /// 带布局结构的 OCR — 支持去斜预处理 + 表格重建。
    /// 默认实现调用 `extract_text_from_image()` 返回纯文本。
    /// `PpOcrProvider` 重写以支持完整的场景能力。
    fn extract_structured(&self, image_path: &Path, _profile: &OcrProfile) -> Result<OcrOutput> {
        let text = self.extract_text_from_image(image_path)?;
        Ok(OcrOutput {
            text,
            table_markdown: None,
            avg_confidence: None,
            lines: None,
            regions: None,
            correction_report: None,
        })
    }

    /// Run the non-text region recognition pass. Default returns empty (plain-OCR providers
    /// opt out). `PpOcrProvider` + the nontext orchestrator override when feature enabled.
    fn recognize_regions(
        &self,
        _image_path: &Path,
        _profile: &OcrProfile,
    ) -> Result<Vec<nontext_regions::Region>> {
        Ok(Vec::new())
    }
}

/// 选择默认 OCR provider —
/// 选择默认 OCR provider — **不触发运行时下载**。
///
/// 设计原则（标准应用规范）:
/// - 模型下载是**安装时**职责（postinst.sh 或 `attune --bootstrap-models` flag）
/// - server 启动路径**不偷偷下载** — 否则用户首次启动等 30s+ 以为卡死
/// - 模型缺失 → 返回 None + diagnostics 端点提示用户跑 bootstrap
///
/// 部署方式覆盖:
/// - apt install attune.deb → postinst.sh 自动下载 (主路径)
/// - cargo binary / 源码部署 → 用户跑 `attune-server-headless --bootstrap-models`
/// - 一键工具 ensure_models_downloaded() 仍在 ppocr.rs (供 bootstrap 调用)
pub fn detect_default_provider() -> Option<Box<dyn OcrProvider>> {
    if let Some(p) = ppocr::detect() {
        log::info!("OCR provider: PP-OCRv5 mobile (ORT)");
        return Some(Box::new(p));
    }
    log::warn!(
        "OCR provider: PP-OCR models missing. Run `attune-server-headless --bootstrap-models` \
         (cargo) or `apt install --reinstall attune` (deb) to download (~21 MB)."
    );
    None
}

/// 抽取 PDF 文字 — 用 pdftoppm 切页 + provider 逐页 OCR。
/// `pdftoppm` 来自 poppler-utils，由 .deb 自动装。
///
/// 等价于 `extract_text_from_pdf_with_dpi(provider, pdf_path, 300)`. 300 DPI 是 PP-OCR
/// 在普通合同/判决书上的最佳实测值；若要换不同 DPI (200 票据 / 600 古籍) 用
/// `extract_text_from_pdf_with_dpi`.
pub fn extract_text_from_pdf(provider: &dyn OcrProvider, pdf_path: &Path) -> Result<String> {
    extract_text_from_pdf_with_dpi(provider, pdf_path, 300)
}

/// 用指定 DPI 抽取 PDF 文字. dpi 通常由 OcrProfile 决定 (200/300/600).
/// 越界值 (< 72 或 > 1200) 退回 300.
pub fn extract_text_from_pdf_with_dpi(
    provider: &dyn OcrProvider,
    pdf_path: &Path,
    dpi: u32,
) -> Result<String> {
    let dpi = if (72..=1200).contains(&dpi) { dpi } else { 300 };
    let pdftoppm = which::which("pdftoppm").map_err(|_| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pdftoppm not found (poppler-utils 应由 .deb 自动装；apt install poppler-utils 修复)",
        ))
    })?;

    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let prefix = tmp.path().join("page");
    let prefix_str = prefix.to_str().ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "non-UTF8 temp path",
        ))
    })?;

    // PDF → 多页 PNG (DPI 由 profile 决定: 200 票据 / 300 合同 / 600 古籍)
    let dpi_str = dpi.to_string();
    let status = Command::new(&pdftoppm)
        .args(["-r", dpi_str.as_str(), "-png"])
        .arg(pdf_path)
        .arg(prefix_str)
        .status()
        .map_err(VaultError::Io)?;
    if !status.success() {
        return Err(VaultError::Io(std::io::Error::other(format!(
            "pdftoppm failed: exit {}",
            status.code().unwrap_or(-1)
        ))));
    }

    let mut pages: Vec<_> = std::fs::read_dir(tmp.path())
        .map_err(VaultError::Io)?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    pages.sort();

    if pages.is_empty() {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pdftoppm produced no pages (PDF may be empty or corrupt)",
        )));
    }

    let mut all = String::with_capacity(pages.len() * 2000);
    let mut failed = 0usize;
    for (idx, png) in pages.iter().enumerate() {
        match provider.extract_text_from_image(png) {
            Ok(text) => {
                all.push_str(text.trim());
                all.push_str("\n\n");
            }
            Err(e) => {
                log::warn!("{} page {} error: {}", provider.name(), idx + 1, e);
                failed += 1;
            }
        }
    }
    log::info!(
        "{} PDF pipeline: {} pages ok, {} failed, {} bytes text",
        provider.name(),
        pages.len() - failed,
        failed,
        all.len()
    );
    Ok(all)
}

/// PDF → 结构化 OCR（带表格重建 + 去斜），按 profile 设置决定行为。
/// 每页单独处理，表格行之间用 `---` 分隔。
pub fn extract_text_from_pdf_with_profile(
    provider: &dyn OcrProvider,
    pdf_path: &Path,
    profile: &OcrProfile,
) -> Result<String> {
    let dpi = if (72..=1200).contains(&profile.dpi) { profile.dpi } else { 300 };
    let pdftoppm = which::which("pdftoppm").map_err(|_| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "pdftoppm not found (poppler-utils 应由 .deb 自动装；apt install poppler-utils 修复)",
        ))
    })?;
    let tmp = tempfile::TempDir::new().map_err(VaultError::Io)?;
    let prefix = tmp.path().join("page");
    let prefix_str = prefix.to_str().ok_or_else(|| {
        VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "non-UTF8 temp path",
        ))
    })?;
    let dpi_str = dpi.to_string();
    let status = Command::new(&pdftoppm)
        .args(["-r", dpi_str.as_str(), "-png"])
        .arg(pdf_path)
        .arg(prefix_str)
        .status()
        .map_err(VaultError::Io)?;
    if !status.success() {
        return Err(VaultError::Io(std::io::Error::other(format!(
            "pdftoppm failed: exit {}",
            status.code().unwrap_or(-1)
        ))));
    }
    let mut pages: Vec<_> = std::fs::read_dir(tmp.path())
        .map_err(VaultError::Io)?
        .filter_map(|r| r.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("png"))
        .collect();
    pages.sort();
    if pages.is_empty() {
        return Err(VaultError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "pdftoppm produced no pages",
        )));
    }
    let mut all = String::with_capacity(pages.len() * 2000);
    let mut failed = 0usize;
    for (idx, png) in pages.iter().enumerate() {
        match provider.extract_structured(png, profile) {
            Ok(out) => {
                if let Some(tbl) = &out.table_markdown {
                    all.push_str(tbl.trim());
                } else {
                    all.push_str(out.text.trim());
                }
                all.push_str("\n\n");
            }
            Err(e) => {
                log::warn!("{} page {} error: {}", provider.name(), idx + 1, e);
                failed += 1;
            }
        }
    }
    log::info!(
        "{} PDF structured pipeline: {} pages ok, {} failed, {} bytes text",
        provider.name(), pages.len() - failed, failed, all.len()
    );
    Ok(all)
}

/// 从注册表按 profile_id 取 OcrProfile. None / 不存在均返回 contract 默认 profile.
pub fn profile_for_id(profile_id: Option<&str>) -> OcrProfile {
    let id = match profile_id {
        Some(s) if !s.is_empty() => s,
        _ => OcrProfile::DEFAULT_ID,
    };
    match profile_registry::ProfileRegistry::load_default() {
        Ok(reg) => reg.get(id).cloned().unwrap_or_else(|| {
            OcrProfile::builtins()
                .into_iter()
                .find(|p| p.id == id)
                .unwrap_or_else(default_contract_profile)
        }),
        Err(_) => default_contract_profile(),
    }
}

/// 从注册表按 profile_id 取 DPI. None / 不存在 / load 失败均返回 300 (默认合同 DPI).
///
/// 这是 OCR pipeline 与 profile 系统的对接点 — 调用方拿到 profile_id 后用此函数
/// 决定 DPI, 再调 `extract_text_from_pdf_with_dpi`.
pub fn dpi_for_profile(profile_id: Option<&str>) -> u32 {
    profile_for_id(profile_id).dpi
}

fn default_contract_profile() -> OcrProfile {
    OcrProfile::builtins()
        .into_iter()
        .find(|p| p.id == "contract")
        .expect("contract builtin always present")
}

/// 自动场景推断 — 按文件名和扩展名启发式推荐 profile_id。
///
/// 规则优先级（低索引优先）：
/// 1. 文件名含表格关键词 → "table"
/// 2. 文件名含表单/申请 → "form"
/// 3. 文件名含票据/发票 → "receipt"
/// 4. 文件名含截图/screenshot → "screenshot"
/// 5. 文件名含证件/名片 → "card"
/// 6. 文件名含古籍/碑文 → "ancient"
/// 7. 无命中 → "contract"（通用高精度扫描件默认）
///
/// 返回值是 profile_id（`&'static str`）。调用方可用 `profile_for_id(Some(id))` 取完整 profile。
pub fn auto_detect_scene(filename: &str) -> &'static str {
    let lower = filename.to_lowercase();
    // 表格
    if lower.contains("table")
        || lower.contains("表格")
        || lower.contains("报表")
        || lower.contains("数据表")
        || lower.contains("excel")
        || lower.contains(".xlsx")
        || lower.contains("sheet")
    {
        return "table";
    }
    // 表单
    if lower.contains("form")
        || lower.contains("表单")
        || lower.contains("申请")
        || lower.contains("登记")
        || lower.contains("问卷")
        || lower.contains("填写")
    {
        return "form";
    }
    // 票据
    if lower.contains("receipt")
        || lower.contains("invoice")
        || lower.contains("票据")
        || lower.contains("发票")
        || lower.contains("收据")
        || lower.contains("流水")
        || lower.contains("账单")
    {
        return "receipt";
    }
    // 截图
    if lower.contains("screenshot")
        || lower.contains("screen")
        || lower.contains("截图")
        || lower.contains("屏幕")
        || lower.contains("capture")
    {
        return "screenshot";
    }
    // 证件 / 名片
    if lower.contains("card")
        || lower.contains("证件")
        || lower.contains("身份证")
        || lower.contains("营业执照")
        || lower.contains("名片")
        || lower.contains("执照")
        || lower.contains("license")
        || lower.contains("passport")
    {
        return "card";
    }
    // 古籍
    if lower.contains("ancient")
        || lower.contains("古籍")
        || lower.contains("碑文")
        || lower.contains("拓片")
        || lower.contains("善本")
    {
        return "ancient";
    }
    // 默认：合同 / 通用扫描件（高精度，含去斜）
    "contract"
}

/// 判断 PDF 是否需要 OCR（pdf_extract 产出文字量低于阈值）
pub fn needs_ocr(extracted_text: &str) -> bool {
    extracted_text.chars().filter(|c| !c.is_whitespace()).count() < 100
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_ocr_threshold() {
        assert!(needs_ocr(""), "empty = needs ocr");
        assert!(needs_ocr("  \n\t"), "whitespace = needs ocr");
        assert!(needs_ocr(&"a".repeat(50)), "50 chars = needs ocr");
        assert!(!needs_ocr(&"a".repeat(200)), "200 chars = enough text");
        let chinese = "中".repeat(100);
        assert!(!needs_ocr(&chinese), "100 Chinese chars = enough");
    }

    #[test]
    fn detect_default_provider_returns_none_when_pp_ocr_missing() {
        // CI 环境无 PP-OCR 模型 → 应返回 None；本测试只确保不 panic
        let _ = detect_default_provider();
    }

    #[test]
    fn dpi_for_profile_empty_returns_default() {
        assert_eq!(dpi_for_profile(None), 300);
        assert_eq!(dpi_for_profile(Some("")), 300);
    }

    #[test]
    fn dpi_for_profile_unknown_returns_default() {
        // unknown profile id → 300 (registry 加载得到 7 builtin 但无此 id)
        // Pin the data dir to a temp dir via the thread-local injection seam rather
        // than overriding HOME/XDG (which leaks process-globally and does NOT isolate
        // on Windows — dirs reads %LOCALAPPDATA%). See platform::set_dir_override_for_test.
        let tmp = tempfile::TempDir::new().expect("tmp");
        let prev = crate::platform::set_dir_override_for_test(Some(tmp.path().to_path_buf()));
        assert_eq!(dpi_for_profile(Some("does-not-exist")), 300);
        crate::platform::set_dir_override_for_test(prev);
    }

    #[test]
    fn auto_detect_scene_table_keywords() {
        assert_eq!(auto_detect_scene("财务报表2024.pdf"), "table");
        assert_eq!(auto_detect_scene("数据表汇总.pdf"), "table");
        assert_eq!(auto_detect_scene("sales_sheet.xlsx"), "table");
        assert_eq!(auto_detect_scene("Q3_excel_export.pdf"), "table");
    }

    #[test]
    fn auto_detect_scene_form_keywords() {
        assert_eq!(auto_detect_scene("入职申请.pdf"), "form");
        assert_eq!(auto_detect_scene("用户登记表.jpg"), "form");
        assert_eq!(auto_detect_scene("questionnaire_form.pdf"), "form");
    }

    #[test]
    fn auto_detect_scene_receipt_keywords() {
        assert_eq!(auto_detect_scene("发票20240501.jpg"), "receipt");
        assert_eq!(auto_detect_scene("银行流水.pdf"), "receipt");
        assert_eq!(auto_detect_scene("invoice_2024.pdf"), "receipt");
    }

    #[test]
    fn auto_detect_scene_screenshot_keywords() {
        assert_eq!(auto_detect_scene("screenshot_20240501.png"), "screenshot");
        assert_eq!(auto_detect_scene("微信截图_001.png"), "screenshot");
    }

    #[test]
    fn auto_detect_scene_card_keywords() {
        assert_eq!(auto_detect_scene("身份证正面.jpg"), "card");
        assert_eq!(auto_detect_scene("营业执照.pdf"), "card");
        assert_eq!(auto_detect_scene("business_card.png"), "card");
        assert_eq!(auto_detect_scene("passport_scan.jpg"), "card");
    }

    #[test]
    fn auto_detect_scene_ancient_keywords() {
        assert_eq!(auto_detect_scene("宋刻古籍.jpg"), "ancient");
        assert_eq!(auto_detect_scene("碑文拓片.png"), "ancient");
    }

    #[test]
    fn auto_detect_scene_default_contract() {
        assert_eq!(auto_detect_scene("劳动合同.pdf"), "contract");
        assert_eq!(auto_detect_scene("扫描件001.pdf"), "contract");
        assert_eq!(auto_detect_scene("document.pdf"), "contract");
    }

    #[test]
    fn auto_detect_scene_case_insensitive() {
        assert_eq!(auto_detect_scene("SCREENSHOT_001.PNG"), "screenshot");
        assert_eq!(auto_detect_scene("Invoice_Q1.PDF"), "receipt");
    }
}

#[cfg(test)]
mod office_types_tests {
    use super::*;

    #[test]
    fn raw_line_serde_roundtrip() {
        let l = RawLine { text: "hi".into(), bbox: BBox { x: 1, y: 2, w: 3, h: 4 }, confidence: 0.9 };
        let s = serde_json::to_string(&l).unwrap();
        let d: RawLine = serde_json::from_str(&s).unwrap();
        assert_eq!(d.text, "hi");
        assert_eq!(d.bbox.x, 1);
        assert_eq!(d.bbox.y, 2);
        assert_eq!(d.bbox.w, 3);
        assert_eq!(d.bbox.h, 4);
        assert!((d.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn bbox_copy_and_clone() {
        let b = BBox { x: 1, y: 2, w: 3, h: 4 };
        let b2 = b; // Copy
        assert_eq!(b2.x, b.x);
    }

    #[test]
    fn ocr_output_new_fields_default_none() {
        let o = OcrOutput {
            text: "x".into(),
            table_markdown: None,
            avg_confidence: None,
            lines: None,
            regions: None,
            correction_report: None,
        };
        assert!(o.regions.is_none());
        assert!(o.correction_report.is_none());
    }

    #[test]
    fn default_recognize_regions_returns_empty() {
        struct Stub;
        impl OcrProvider for Stub {
            fn name(&self) -> &str {
                "stub"
            }
            fn has_chinese(&self) -> bool {
                false
            }
            fn extract_text_from_image(&self, _: &Path) -> Result<String> {
                Ok(String::new())
            }
        }
        let out = Stub.recognize_regions(
            Path::new("/x.png"),
            &crate::ocr::profile::OcrProfile::default(),
        );
        assert!(out.unwrap().is_empty(), "default impl must return empty regions");
    }
}
