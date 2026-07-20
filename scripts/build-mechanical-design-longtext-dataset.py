#!/usr/bin/env python3
"""Build a long-text KB benchmark manifest from handbook-of-mechanical-design.

The source repository stores five Chinese PDF volumes via Git LFS. This script
keeps Attune small by writing a deterministic JSON manifest. Use --materialize
only on the benchmark host that will bind/index the corpus.

Usage:
  python3 scripts/build-mechanical-design-longtext-dataset.py
  python3 scripts/build-mechanical-design-longtext-dataset.py --materialize
  python3 scripts/build-mechanical-design-longtext-dataset.py --golden-out rust/tests/golden/mechanical_design_queries.json
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable


REPO_ROOT = Path(__file__).resolve().parent.parent

SOURCE_REPO_URL = "https://github.com/GEQfa/handbook-of-mechanical-design.git"
SOURCE_WEB_URL = "https://github.com/GEQfa/handbook-of-mechanical-design"
DEFAULT_COMMIT = "86832fd643cb1f9cfa1188d242d34b62dd52e41f"
DEFAULT_REPO_DIR = Path(
    os.environ.get("MECHANICAL_DESIGN_HANDBOOK_ROOT", "/data/corpora/handbook-of-mechanical-design")
)
DEFAULT_OUT = REPO_ROOT / "tests/e2e/mechanical_design_longtext_cases.json"

PINNED_LFS_OBJECTS = {
    "机械设计手册 第六版 第1卷.PDF": {
        "size": 59401402,
        "oid": "229084e447550280d68d56f64a857ecf3cbbb34827e92466453d87ed9de70e8e",
        "topics": ["常用设计资料", "机械制图", "工程材料", "机构"],
    },
    "机械设计手册 第六版 第2卷.PDF": {
        "size": 80796627,
        "oid": "d28e7168fa6345e2f1126e359ddecef35f5dcc0cb979549579cdf996a0443534",
        "topics": ["连接", "紧固件", "轴", "弹簧", "轴承"],
    },
    "-机械设计手册 第六版 第3卷.PDF": {
        "size": 43791161,
        "oid": "797ae318ba3f18d13a86070d6cae3bc9245d0d783ee47e8e9aaeb3955fac6e4f",
        "topics": ["齿轮传动", "带传动", "链传动", "蜗杆传动"],
    },
    "-机械设计手册 第六版 第4卷.PDF": {
        "size": 49879781,
        "oid": "6f139eeda70d14fc031af3e5052a9e95b3476f64d1071ea00296bde0eb9b55db",
        "topics": ["液压传动", "气压传动", "润滑", "密封"],
    },
    "-机械设计手册 第六版 第5卷.PDF": {
        "size": 68823448,
        "oid": "a16180d726afb9cf047048b55b7015f3c3fd5b6b584b0a701b7dd639ac731b73",
        "topics": ["机械控制", "机电一体化", "现代设计", "可靠性"],
    },
}

PINNED_PATHS = list(PINNED_LFS_OBJECTS)


@dataclass(frozen=True)
class TreeEntry:
    path: str
    size: int | None = None
    sha: str | None = None
    lfs_oid: str | None = None


@dataclass(frozen=True)
class Doc:
    id: str
    path: str
    title: str
    volume: int
    manual_type: str
    size_bytes: int | None
    lfs_oid: str | None
    tags: list[str]
    partition: dict[str, str]
    long_text_tier: str


def run(cmd: list[str], cwd: Path | None = None) -> str:
    env = os.environ.copy()
    env["GIT_TERMINAL_PROMPT"] = "0"
    env.setdefault("GIT_CONFIG_GLOBAL", os.devnull)
    env.setdefault("GIT_CONFIG_SYSTEM", os.devnull)
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return proc.stdout


def run_with_input(cmd: list[str], cwd: Path, text: str) -> str:
    env = os.environ.copy()
    env["GIT_TERMINAL_PROMPT"] = "0"
    env.setdefault("GIT_CONFIG_GLOBAL", os.devnull)
    env.setdefault("GIT_CONFIG_SYSTEM", os.devnull)
    proc = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        input=text,
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    return proc.stdout


def ensure_repo(repo_dir: Path, commit: str) -> None:
    if repo_dir.exists() and not (repo_dir / ".git").exists():
        raise SystemExit(f"{repo_dir} exists but is not a git repository")

    repo_dir.parent.mkdir(parents=True, exist_ok=True)
    if not repo_dir.exists():
        print(f"[mechanical-design-kb] cloning metadata into {repo_dir}", file=sys.stderr)
        run(["git", "clone", "--filter=blob:none", "--no-checkout", SOURCE_REPO_URL, str(repo_dir)])

    try:
        run(["git", "cat-file", "-e", f"{commit}^{{commit}}"], cwd=repo_dir)
    except subprocess.CalledProcessError:
        print(f"[mechanical-design-kb] fetching pinned commit {commit}", file=sys.stderr)
        run(["git", "fetch", "--filter=blob:none", "origin", commit], cwd=repo_dir)


def parse_lfs_pointer(text: str) -> tuple[str | None, int | None]:
    oid: str | None = None
    size: int | None = None
    for line in text.splitlines():
        if line.startswith("oid sha256:"):
            oid = line.removeprefix("oid sha256:").strip()
        elif line.startswith("size "):
            try:
                size = int(line.removeprefix("size ").strip())
            except ValueError:
                size = None
    return oid, size


def static_tree() -> dict[str, TreeEntry]:
    entries: dict[str, TreeEntry] = {}
    for path, meta in PINNED_LFS_OBJECTS.items():
        entries[path] = TreeEntry(
            path=path,
            size=int(meta["size"]),
            lfs_oid=str(meta["oid"]),
        )
    return entries


def local_tree(repo_dir: Path, commit: str) -> dict[str, TreeEntry]:
    raw = run(["git", "ls-tree", "-r", "-l", commit], cwd=repo_dir)
    entries = static_tree()
    for line in raw.splitlines():
        parts = line.split(None, 4)
        if len(parts) != 5 or parts[1] != "blob":
            continue
        sha = parts[2]
        size_raw = parts[3]
        path = parts[4].strip('"')
        try:
            decoded_path = path.encode("utf-8").decode("unicode_escape").encode("latin1").decode("utf-8")
        except Exception:
            decoded_path = path
        size = None if size_raw == "-" else int(size_raw)
        lfs_oid = PINNED_LFS_OBJECTS.get(decoded_path, {}).get("oid")
        if size is not None and size <= 1024:
            try:
                pointer = run(["git", "show", sha], cwd=repo_dir)
                parsed_oid, parsed_size = parse_lfs_pointer(pointer)
                lfs_oid = parsed_oid or lfs_oid
                size = parsed_size or PINNED_LFS_OBJECTS.get(decoded_path, {}).get("size") or size
            except subprocess.CalledProcessError:
                pass
        entries[decoded_path] = TreeEntry(
            path=decoded_path,
            size=int(size) if isinstance(size, int) else None,
            sha=sha,
            lfs_oid=str(lfs_oid) if lfs_oid else None,
        )
    return entries


def slug(text: str) -> str:
    text = text.lower()
    text = re.sub(r"[^a-z0-9]+", "-", text)
    return text.strip("-")[:96]


def volume_from_path(path: str) -> int:
    match = re.search(r"第(\d+)卷", path)
    if not match:
        raise ValueError(f"cannot infer volume from path: {path}")
    return int(match.group(1))


def long_text_tier(size: int | None) -> str:
    if size is None:
        return "unknown"
    mib = size / 1024 / 1024
    if mib >= 75:
        return "xxl"
    if mib >= 30:
        return "xl"
    if mib >= 10:
        return "l"
    if mib >= 1:
        return "m"
    return "s"


def make_doc(entry: TreeEntry) -> Doc:
    volume = volume_from_path(entry.path)
    meta = PINNED_LFS_OBJECTS.get(entry.path, {})
    topics = [str(topic) for topic in meta.get("topics", [])]
    title = f"机械设计手册 第六版 第{volume}卷"
    doc_id = f"mechanical_design_volume_{volume}"
    tags = [
        "mechanical-design-handbook",
        "chinese",
        "pdf",
        "ocr-required",
        f"volume-{volume}",
        long_text_tier(entry.size),
        *[slug(topic) for topic in topics],
    ]
    partition = {
        "domain": "mechanical_design",
        "language": "zh",
        "manual_type": "handbook",
        "volume": str(volume),
    }
    return Doc(
        id=doc_id,
        path=entry.path,
        title=title,
        volume=volume,
        manual_type="HANDBOOK",
        size_bytes=entry.size,
        lfs_oid=entry.lfs_oid,
        tags=[tag for tag in tags if tag],
        partition=partition,
        long_text_tier=long_text_tier(entry.size),
    )


def select_docs(entries: dict[str, TreeEntry], limit: int) -> list[Doc]:
    selected = [make_doc(entries[path]) for path in PINNED_PATHS if path in entries]
    selected.sort(key=lambda doc: doc.volume)
    return selected[:limit]


def doc_map(docs: Iterable[Doc]) -> dict[int, Doc]:
    return {doc.volume: doc for doc in docs}


def query(
    docs_by_volume: dict[int, Doc],
    qid: str,
    text: str,
    volumes: list[int],
    expect_any: list[str],
    category: str,
    difficulty: str,
    partition: dict[str, str] | None = None,
    min_hit_in_top_k: int = 5,
    expected_behavior: str = "retrieve_and_answer_with_citations",
) -> dict | None:
    present_docs = [docs_by_volume[volume] for volume in volumes if volume in docs_by_volume]
    if not present_docs:
        return None
    out = {
        "id": qid,
        "q": text,
        "query": text,
        "category": category,
        "difficulty": difficulty,
        "acceptable_hits": [doc.id for doc in present_docs],
        "acceptable_files": [doc.path for doc in present_docs],
        "expect_any": expect_any,
        "min_hit_in_top_k": min_hit_in_top_k,
        "expected_behavior": expected_behavior,
    }
    if partition:
        out["partition_expectation"] = partition
    return out


def build_queries(docs: list[Doc]) -> list[dict]:
    d = doc_map(docs)
    specs = [
        query(
            d,
            "mdh_volume_1_overview",
            "机械设计手册第六版第1卷主要覆盖哪些基础设计资料、机械制图或工程材料内容？请给出引用。",
            [1],
            ["第1卷", "机械设计", "材料", "制图"],
            "volume_overview",
            "easy",
            {"domain": "mechanical_design", "volume": "1"},
            3,
        ),
        query(
            d,
            "mdh_volume_2_connection_spring_bearing",
            "查找机械设计手册中连接、紧固件、弹簧、轴承或轴类零件的资料，说明应优先引用哪一卷。",
            [2],
            ["第2卷", "连接", "紧固件", "弹簧", "轴承"],
            "component_lookup",
            "easy",
            {"domain": "mechanical_design", "volume": "2"},
            3,
        ),
        query(
            d,
            "mdh_gear_transmission_design",
            "齿轮传动设计参数、强度校核或传动选型资料应查机械设计手册第几卷？回答必须带引用。",
            [3],
            ["第3卷", "齿轮", "传动", "强度"],
            "topic_lookup",
            "medium",
            {"domain": "mechanical_design", "volume": "3"},
            3,
        ),
        query(
            d,
            "mdh_belt_chain_vs_gear",
            "对比齿轮传动、带传动和链传动在选型检索时应查哪些资料，回答要区分主题并引用来源。",
            [3],
            ["齿轮", "带传动", "链传动", "第3卷"],
            "mixed_difficulty_chat",
            "medium",
            {"domain": "mechanical_design", "volume": "3"},
            5,
        ),
        query(
            d,
            "mdh_hydraulic_pneumatic_lookup",
            "液压传动、气压传动、润滑或密封相关资料在机械设计手册中应查哪一卷？",
            [4],
            ["第4卷", "液压", "气压", "润滑", "密封"],
            "topic_lookup",
            "medium",
            {"domain": "mechanical_design", "volume": "4"},
            5,
        ),
        query(
            d,
            "mdh_control_mechatronics_reliability",
            "机械控制、机电一体化、现代设计方法或可靠性设计应优先检索机械设计手册哪一卷？",
            [5],
            ["第5卷", "控制", "机电", "可靠性"],
            "topic_lookup",
            "medium",
            {"domain": "mechanical_design", "volume": "5"},
            5,
        ),
        query(
            d,
            "mdh_cross_volume_shaft_bearing_transmission",
            "我要设计一套含轴、轴承和齿轮传动的机械系统，应该跨哪些卷检索？请分步骤说明并引用来源。",
            [2, 3],
            ["轴", "轴承", "齿轮", "第2卷", "第3卷"],
            "cross_document",
            "hard",
            {"domain": "mechanical_design", "volume_any": "2,3"},
            10,
        ),
        query(
            d,
            "mdh_cross_volume_hydraulic_control",
            "如果机械系统同时涉及液压执行机构和电控/机电一体化控制，应怎样在手册第4卷和第5卷之间分工检索？",
            [4, 5],
            ["液压", "控制", "机电", "第4卷", "第5卷"],
            "cross_document",
            "hard",
            {"domain": "mechanical_design", "volume_any": "4,5"},
            10,
        ),
        query(
            d,
            "mdh_table_formula_ocr_probe",
            "请检索机械设计手册中表格、公式或设计参数密集的内容，并说明 OCR 结果是否足以支撑引用。",
            [1, 2, 3, 4, 5],
            ["表", "公式", "参数", "机械设计手册"],
            "ocr_table_formula_probe",
            "hard",
            {"domain": "mechanical_design"},
            10,
        ),
        query(
            d,
            "mdh_all_volumes_inventory",
            "机械设计手册第六版 PDF 语料库当前入库了哪些卷册？按卷号列出并给出引用。",
            [1, 2, 3, 4, 5],
            ["第1卷", "第2卷", "第3卷", "第4卷", "第5卷"],
            "inventory",
            "easy",
            {"domain": "mechanical_design"},
            10,
        ),
    ]
    return [q for q in specs if q is not None]


def doc_to_json(doc: Doc) -> dict:
    return {
        "id": doc.id,
        "file": doc.path,
        "title": doc.title,
        "manufacturer": "机械工业出版社",
        "aircraft": "not_applicable",
        "manual_type": doc.manual_type,
        "volume": doc.volume,
        "size_bytes": doc.size_bytes,
        "lfs_oid": doc.lfs_oid,
        "long_text_tier": doc.long_text_tier,
        "tags": doc.tags,
        "index_partition": doc.partition,
    }


def build_manifest(docs: list[Doc], queries: list[dict], args: argparse.Namespace) -> dict:
    source_root = str(args.repo_dir)
    total_bytes = sum(doc.size_bytes or 0 for doc in docs)
    query_ids = {q["id"] for q in queries}
    web_e2e_query_ids = [
        qid
        for qid in [
            "mdh_gear_transmission_design",
            "mdh_cross_volume_shaft_bearing_transmission",
            "mdh_table_formula_ocr_probe",
        ]
        if qid in query_ids
    ]
    multiturn_query = "mdh_gear_transmission_design" if "mdh_gear_transmission_design" in query_ids else None
    return {
        "_doc": "Long-text KB benchmark manifest for Chinese mechanical-design handbook PDFs. The source PDFs are Git LFS objects and require OCR for scanned pages.",
        "_version": "2026-07-20",
        "source_root": source_root,
        "source_root_env": "MECHANICAL_DESIGN_HANDBOOK_ROOT",
        "source": {
            "repo_url": SOURCE_REPO_URL,
            "web_url": SOURCE_WEB_URL,
            "commit": args.commit,
            "license_file": None,
            "lfs_required": True,
            "safety_note": "Dataset is for retrieval, OCR, and grounded-answer benchmarking; answers must cite source pages and avoid unsupported design sign-off claims.",
        },
        "selection": {
            "generated_by": "scripts/build-mechanical-design-longtext-dataset.py",
            "limit_docs": args.limit_docs,
            "documents_count": len(docs),
            "queries_count": len(queries),
            "total_selected_bytes": total_bytes,
            "known_size_documents": sum(1 for doc in docs if doc.size_bytes is not None),
            "materialize_command": (
                "python3 scripts/build-mechanical-design-longtext-dataset.py "
                f"--repo-dir {source_root} --materialize --limit-docs {args.limit_docs}"
            ),
            "profiles": {
                "smoke": {
                    "documents": [doc.id for doc in docs[:2]],
                    "purpose": "fast Chinese PDF/LFS/OCR ingestion sanity check",
                },
                "edge_scheduler_30b": {
                    "documents": [doc.id for doc in docs[:5]],
                    "purpose": "all five volumes with OCR, vector generation, grounded chat, and multi-turn source continuity",
                    "max_retrieved_context_docs": 4,
                    "max_final_context_chunks": 12,
                },
                "edge_scheduler_comprehensive": {
                    "documents": [doc.id for doc in docs[:5]],
                    "purpose": "full mechanical-design handbook E2E parallel to airplane GitHub corpus",
                    "max_retrieved_context_docs": 6,
                    "max_final_context_chunks": 18,
                },
                "stress": {
                    "documents": [doc.id for doc in docs],
                    "purpose": "repeat chat, OCR-heavy PDFs, cross-volume retrieval pressure",
                },
            },
        },
        "coverage_dimensions": [
            "git_lfs_materialization",
            "chinese_scanned_pdf_ocr",
            "table_formula_dense_pages",
            "exact_volume_lookup",
            "mechanical_component_topic_lookup",
            "cross_volume_retrieval",
            "mixed_difficulty_chat",
            "multiturn_source_continuity",
            "vector_generation_and_refresh",
        ],
        "web_e2e": {
            "default_query_id": web_e2e_query_ids[0] if web_e2e_query_ids else None,
            "query_ids": web_e2e_query_ids,
            "expected_surface": [
                "indexed document visible in Items view",
                "chat input accepts Chinese long-text KB query",
                "assistant answer contains expected Chinese mechanical-design terms",
                "citation chips are rendered",
                "scheduler status is rendered when the edge scheduler path is used",
            ],
        },
        "multiturn": {
            "default_query_id": multiturn_query,
            "forbidden_followup_terms": [
                "a320",
                "airbus",
                "boeing",
                "qrh",
                "fcom",
                "飞机",
                "飞行",
            ],
            "turns": [
                {
                    "turn_id": "initial_grounded_answer",
                    "message_from_query": True,
                },
                {
                    "turn_id": "history_source_continuity",
                    "message": "继续只基于上一轮引用的机械设计手册卷册回答：该来源属于哪一卷，主题是什么？不要切换到航空手册或其它资料。",
                    "expected_terms": ["机械设计手册", "第3卷", "齿轮", "传动"],
                    "reject_forbidden_sources": True,
                },
                {
                    "turn_id": "hard_followup_retrieval_path",
                    "message": "在上一轮基础上，给出齿轮传动设计参数或强度校核资料的检索路径；只说明检索依据和引用来源，不给出工程签核结论。",
                    "expected_terms": ["齿轮", "传动", "强度", "引用"],
                    "reject_forbidden_sources": True,
                },
            ],
        },
        "evaluation_targets": {
            "vector_search": {
                "hit_at_5_min": 0.80,
                "recall_at_10_min": 0.75,
                "mrr_at_10_min": 0.60,
                "partition_hit_rate_min": 0.90,
                "warm_p50_latency_ms_max": 1200,
                "warm_p95_latency_ms_max": 3500,
            },
            "rag_answer": {
                "answer_accuracy_rate_min": 0.75,
                "citation_hit_rate_min": 0.85,
                "unsafe_operational_advice_rate_max": 0.0,
                "edge_scheduler_30b_p95_latency_ms_max": 15000,
            },
            "context_admission": {
                "edge_scheduler_30b_max_context_documents": 4,
                "edge_scheduler_30b_max_final_chunks": 12,
                "edge_scheduler_comprehensive_max_context_documents": 6,
                "edge_scheduler_comprehensive_max_final_chunks": 18,
            },
            "stability": {
                "repeat_chat_runs_min": 3,
                "terminal_error_rate_max": 0.02,
                "chat_latency_cv_max": 0.35,
            },
        },
        "documents": [doc_to_json(doc) for doc in docs],
        "queries": queries,
        "pro_blocked_cases": [
            "engineering-design-signoff",
            "safety-critical-load-certification",
            "manufacturing-release-decision",
            "unsupported-table-or-formula-extrapolation",
        ],
    }


def build_golden(manifest: dict) -> dict:
    return {
        "_doc": "Golden retrieval queries generated from mechanical_design_longtext_cases.json.",
        "_version": manifest["_version"],
        "_corpus_pins": {
            "mechanical-design-handbook": f"commit:{manifest['source']['commit']}",
        },
        "scenarios": [
            {
                "id": "MECH-LONG",
                "name": "Mechanical design handbook / Chinese OCR long-text KB",
                "corpus": "mechanical-design-handbook",
                "queries": [
                    {
                        "id": q["id"],
                        "query": q["query"],
                        "difficulty": q.get("difficulty"),
                        "acceptable_hits": q["acceptable_hits"],
                        "min_hit_in_top_k": q["min_hit_in_top_k"],
                    }
                    for q in manifest["queries"]
                ],
            }
        ],
    }


def materialize(repo_dir: Path, commit: str, docs: list[Doc]) -> None:
    paths = [doc.path for doc in docs]
    run(["git", "sparse-checkout", "init", "--no-cone"], cwd=repo_dir)
    run_with_input(["git", "sparse-checkout", "set", "--stdin"], cwd=repo_dir, text="\n".join(paths))
    run(["git", "checkout", "--force", commit], cwd=repo_dir)

    try:
        run(["git", "lfs", "version"], cwd=repo_dir)
    except (FileNotFoundError, subprocess.CalledProcessError) as exc:
        raise SystemExit("git-lfs is required to materialize handbook PDF objects") from exc

    run(["git", "lfs", "install", "--local"], cwd=repo_dir)
    run(["git", "lfs", "pull", "--include", ",".join(paths)], cwd=repo_dir)


def is_lfs_pointer(path: Path) -> bool:
    if not path.is_file() or path.stat().st_size > 1024:
        return False
    try:
        head = path.read_text(encoding="utf-8", errors="ignore")[:256]
    except OSError:
        return False
    return head.startswith("version https://git-lfs.github.com/spec/v1")


def verify_materialized(repo_dir: Path, docs: list[Doc]) -> None:
    missing: list[str] = []
    pointers: list[str] = []
    too_small: list[str] = []
    for doc in docs:
        path = repo_dir / doc.path
        if not path.exists():
            missing.append(doc.path)
            continue
        if is_lfs_pointer(path):
            pointers.append(doc.path)
            continue
        if path.is_file() and doc.size_bytes and path.stat().st_size < min(doc.size_bytes // 2, 1024 * 1024):
            too_small.append(doc.path)
    if missing or pointers or too_small:
        details = {
            "missing": missing[:3],
            "lfs_pointers": pointers[:3],
            "too_small": too_small[:3],
        }
        raise SystemExit(f"materialized checkout did not contain real PDF objects: {details}")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo-dir", type=Path, default=DEFAULT_REPO_DIR)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--golden-out", type=Path, default=None)
    parser.add_argument("--commit", default=DEFAULT_COMMIT)
    parser.add_argument("--limit-docs", type=int, default=5)
    parser.add_argument("--materialize", action="store_true")
    parser.add_argument(
        "--no-github-api",
        action="store_true",
        help="kept for parity with the airplane builder; this builder uses pinned LFS metadata",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.materialize:
        ensure_repo(args.repo_dir, args.commit)
        entries = local_tree(args.repo_dir, args.commit)
    elif args.repo_dir.exists() and (args.repo_dir / ".git").exists():
        entries = local_tree(args.repo_dir, args.commit)
    else:
        entries = static_tree()

    docs = select_docs(entries, args.limit_docs)
    queries = build_queries(docs)
    manifest = build_manifest(docs, queries, args)

    if args.materialize:
        materialize(args.repo_dir, args.commit, docs)
        verify_materialized(args.repo_dir, docs)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"[mechanical-design-kb] wrote {args.out}", file=sys.stderr)

    if args.golden_out:
        golden = build_golden(manifest)
        args.golden_out.parent.mkdir(parents=True, exist_ok=True)
        args.golden_out.write_text(json.dumps(golden, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        print(f"[mechanical-design-kb] wrote {args.golden_out}", file=sys.stderr)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
