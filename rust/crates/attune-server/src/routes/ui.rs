use axum::http::header;
use axum::response::IntoResponse;

// 前端由 `attune-server/ui/` 子项目（Preact + Vite）产出单文件 HTML
// 修改 UI 后需 `cd ui && npm run build` 重新生成 dist/index.html
const INDEX_HTML: &str = include_str!("../../ui/dist/index.html");

pub async fn index() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        INDEX_HTML,
    )
}
