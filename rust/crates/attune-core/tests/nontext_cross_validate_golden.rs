//! §9.1 OCR-纠错 golden: deterministically-constructed cases where PP-OCR's text differs
//! from the (human-GT) corrected value. Validates cross-validation FLAGS the conflict
//! (does NOT auto-apply). GT is independent of any recognizer (Agent 验证铁律): each case
//! is a hand-authored (ocr, gt) pair with a known expected `Agreement` — the harness never
//! calls the recognizer to compute its own ground truth.
#![cfg(feature = "nontext")]

use attune_core::ocr::nontext::cross_validate::compare_content;
use attune_core::ocr::nontext::Agreement;

struct Case {
    ocr: &'static str,
    gt: &'static str,
    expect: Agreement,
}

/// Known OCR-error pairs (visual-confusable digits/letters) + a few true-agreement
/// sentinels. The conflict set is the high-value signal: OCR cannot self-detect these,
/// so a second opinion must flag them. Cases are real OCR confusions, not synthetic noise.
fn cases() -> Vec<Case> {
    use Agreement::*;
    vec![
        // ── ContentConflict: classic OCR confusions (≥8 required) ──
        Case { ocr: "1OO", gt: "100", expect: ContentConflict }, // O↔0
        Case { ocr: "l23", gt: "123", expect: ContentConflict }, // l↔1
        Case { ocr: "I5", gt: "15", expect: ContentConflict },   // I↔1
        Case { ocr: "5O0", gt: "500", expect: ContentConflict }, // O↔0
        Case { ocr: "B8", gt: "88", expect: ContentConflict },   // B↔8
        Case { ocr: "rn", gt: "m", expect: ContentConflict },    // rn↔m
        Case { ocr: "S5", gt: "55", expect: ContentConflict },   // S↔5
        Case { ocr: "Z2", gt: "22", expect: ContentConflict },   // Z↔2
        Case { ocr: "g9", gt: "99", expect: ContentConflict },   // g↔9
        Case { ocr: "¥1,OOO", gt: "¥1,000", expect: ContentConflict },
        // ── Agree: true matches (sentinels — must NOT be flagged) ──
        Case { ocr: "2024-01-15", gt: "2024-01-15", expect: Agree },
        Case { ocr: "合同编号", gt: "合同编号", expect: Agree },
    ]
}

#[test]
fn ocr_correction_golden_flags_known_confusions() {
    let mut conflicts = 0usize;
    let mut agreements = 0usize;
    for c in cases() {
        let got = compare_content(c.ocr, c.gt);
        assert_eq!(got, c.expect, "ocr={:?} gt={:?}", c.ocr, c.gt);
        match got {
            Agreement::ContentConflict => conflicts += 1,
            Agreement::Agree => agreements += 1,
            Agreement::StructureDiscrepancy => {}
        }
    }
    // R5 invariant: cross-validation FLAGS conflicts; the ≥8 floor ratchets (only-up).
    assert!(
        conflicts >= 8,
        "expected >=8 known OCR-error conflicts in the golden set, got {conflicts}"
    );
    // Sentinels prove we do not over-flag true matches.
    assert!(agreements >= 2, "expected >=2 true-agreement sentinels, got {agreements}");
}

/// R5: cross-validation must NEVER silently rewrite — `compare_content` only classifies,
/// it returns an `Agreement` and has no side effect on the inputs.
#[test]
fn compare_content_is_pure_no_autocorrect() {
    let ocr = "1OO";
    let gt = "100";
    // Calling twice yields the same classification (deterministic, no mutation).
    assert_eq!(compare_content(ocr, gt), compare_content(ocr, gt));
    // The conflict is reported, not resolved — the function cannot return the "corrected"
    // value, only the disagreement, so a human/accept step stays in the loop (spec §2.2).
    assert_eq!(compare_content(ocr, gt), Agreement::ContentConflict);
}
