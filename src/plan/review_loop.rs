use crate::plan::translator::TranslatedPlan;

#[derive(Debug)]
pub enum SpecReviewOutcome {
    Approved,
    Question {
        question_id: String,
        question: String,
    },
}

pub fn review_spec(markdown: &str, plan: &TranslatedPlan) -> SpecReviewOutcome {
    if markdown.contains("???") || markdown.contains("[QUESTION]") {
        return SpecReviewOutcome::Question {
            question_id: "spec-q-1".to_string(),
            question: "Spec contains ambiguity marker (??? or [QUESTION]). Please clarify expected behavior.".to_string(),
        };
    }

    if plan.tasks.iter().any(|t| t.objective.trim().is_empty()) {
        return SpecReviewOutcome::Question {
            question_id: "spec-q-2".to_string(),
            question: "At least one task objective is empty. Please clarify objective.".to_string(),
        };
    }

    SpecReviewOutcome::Approved
}
