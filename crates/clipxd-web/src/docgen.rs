//! Video→document workflows: turn a clip's index into a real markdown document, not just a
//! title/tl;dr. Same shared input as [`deeppass`](crate::deeppass) (timestamped
//! transcript+captions+OCR), same [`crate::llm`] NVIDIA/Gemini-fallback backend, different
//! prompt per document shape. Generated **on request** (`GET /clip/:id/doc/:kind`), not on
//! the background enrichment path — a doc type is a per-ask output, not a property of the
//! clip the way title/tl;dr/chapters are.

use crate::{deeppass, llm};
use anyhow::{bail, Result};
use clipxd_index::Index;

/// The document shapes this pitches to. Each corresponds to one downstream workflow: opening
/// a PR that changed the thing being recorded, writing up how to reproduce what happened, or
/// turning a manual-test walkthrough into a repeatable QA checklist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocKind {
    PrDescription,
    Sop,
    QaSteps,
}

impl DocKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pr-description" | "pr" => Some(Self::PrDescription),
            "sop" => Some(Self::Sop),
            "qa-steps" | "qa" => Some(Self::QaSteps),
            _ => None,
        }
    }

    pub fn slug(&self) -> &'static str {
        match self {
            Self::PrDescription => "pr-description",
            Self::Sop => "sop",
            Self::QaSteps => "qa-steps",
        }
    }

    fn title(&self) -> &'static str {
        match self {
            Self::PrDescription => "PR description",
            Self::Sop => "Standard Operating Procedure",
            Self::QaSteps => "QA test steps",
        }
    }

    fn instructions(&self) -> &'static str {
        match self {
            Self::PrDescription => {
                "Write a GitHub pull request description in markdown, grounded ONLY in what the \
                 index below shows (never invent files, function names, or a diff you can't see). \
                 Structure: a `## Summary` section (2-4 bullet points, present tense, on what \
                 changed/was demonstrated), then a `## Test plan` section as a markdown checklist \
                 (`- [ ] ...`) of what to verify, derived from what was actually clicked/typed/\
                 navigated in the recording and any errors that appeared. If the recording shows \
                 an error being fixed or a feature being exercised, name it concretely (verbatim \
                 error text, verbatim button/field labels) rather than paraphrasing vaguely."
            }
            Self::Sop => {
                "Write a Standard Operating Procedure in markdown that lets someone else reproduce \
                 exactly what happens in this recording, in order. Structure: a one-line `# ` title \
                 naming the procedure, then a numbered list of steps (`1. `, `2. `, ...), each step \
                 one concrete action (navigate/click/type/verify), quoting on-screen text/labels \
                 verbatim where the index shows them. End with an `## Expected result` section \
                 describing the end state (what should be true when done). If an error appeared in \
                 the recording, add a `## Troubleshooting` section naming it verbatim and what \
                 (if anything) the index shows was done about it."
            }
            Self::QaSteps => {
                "Write a QA test-case checklist in markdown derived from this recording. Structure: \
                 a one-line `# ` title naming what's being tested, a `## Preconditions` line if the \
                 index implies any (e.g. a specific starting screen), then numbered test steps as a \
                 markdown checklist (`1. [ ] Do X` — action first, verification second on the same \
                 line where possible), quoting on-screen labels/errors verbatim. Cover the actual \
                 path shown, not a generic template — if the recording shows a specific error \
                 (e.g. a failed request), include a step asserting that error's exact text appears, \
                 not just \"verify error handling works\"."
            }
        }
    }
}

const PROMPT_PREFIX: &str = "You are turning a screen recording's already-extracted index (timestamped scene \
captions, on-screen OCR text, and any transcribed speech) into a real document for a developer's workflow. \
Output ONLY the markdown document itself — no commentary before or after, no explanation of what you did.\n\n";

/// Generate a `kind` document for `idx` (an already-loaded index — the caller goes through
/// the storage abstraction, e.g. `load_index`, so this works the same in local and S3-hosted
/// mode). Never merges into `index.json` — callers get the markdown text back directly
/// (`GET /clip/:id/doc/:kind`'s response body).
pub async fn generate(idx: &Index, kind: DocKind) -> Result<String> {
    if !llm::any_backend_configured() {
        bail!("no LLM backend configured (set NVIDIA_API_KEY or GEMINI_API_KEY)");
    }
    let context = deeppass::build_context(idx);
    if context.trim().is_empty() {
        bail!("no transcript/OCR/captions yet to generate a {} from", kind.title());
    }
    let prompt = format!("{PROMPT_PREFIX}{}\n\nINDEX DATA:\n{context}", kind.instructions());
    let (text, used) = llm::complete(&prompt, false).await?;
    eprintln!("docgen ({used}): generated {} for {}", kind.slug(), idx.id);
    Ok(llm::strip_fence(&text).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doc_kind_parses_slugs_and_aliases() {
        assert_eq!(DocKind::parse("pr-description"), Some(DocKind::PrDescription));
        assert_eq!(DocKind::parse("pr"), Some(DocKind::PrDescription));
        assert_eq!(DocKind::parse("sop"), Some(DocKind::Sop));
        assert_eq!(DocKind::parse("qa-steps"), Some(DocKind::QaSteps));
        assert_eq!(DocKind::parse("qa"), Some(DocKind::QaSteps));
        assert_eq!(DocKind::parse("nonsense"), None);
    }

    #[test]
    fn slug_roundtrips_through_parse() {
        for kind in [DocKind::PrDescription, DocKind::Sop, DocKind::QaSteps] {
            assert_eq!(DocKind::parse(kind.slug()), Some(kind));
        }
    }
}
