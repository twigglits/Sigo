//! Bridges backend [`QuestionRequest`]s to the terminal picker through the
//! SOP translation layer.
//!
//! Claude asks its clarification questions in Chinese (the conversation is
//! Chinese); this bridge translates question/header/option fields ZH→EN for
//! display — falling back to the raw Chinese per field when the local
//! translator fails, the same graceful degradation as the response path —
//! and maps the user's choice back to what the model expects:
//!
//! - picked options ⇒ the **original untranslated labels** (the CLI matches
//!   answers byte-for-byte against the model's own words), ", "-joined for
//!   multi-select;
//! - free text ⇒ sanitized, then EN→ZH through the translator (the standard
//!   outbound SOP). Unlike display translation this direction never degrades
//!   silently: on failure the question is declined with a visible error.

use std::sync::{Arc, Mutex as StdMutex};

use indicatif::ProgressBar;
use sigo_core::{
    sanitize, AnyTranslator, AskQuestion, Direction, QuestionAnswer, QuestionReply,
    QuestionRequest, Translator,
};
use tokio::sync::mpsc;

use crate::picker::{pick, DisplayOption, DisplayQuestion, PickOutcome};

/// Shared slot for the live translator (REPL slash-commands hot-swap it).
pub type SharedTranslator = Arc<StdMutex<AnyTranslator>>;
/// Shared slot for the active turn's spinner, suspended while asking.
pub type SpinnerSlot = Arc<StdMutex<Option<ProgressBar>>>;

/// Translate one field ZH→EN for display; the raw text is kept on error or
/// when the field is empty.
async fn display_field(translator: &AnyTranslator, zh: &str) -> String {
    if zh.trim().is_empty() {
        return zh.to_string();
    }
    match translator.translate(zh, Direction::ZhToEn).await {
        Ok(en) => en,
        Err(_) => zh.to_string(),
    }
}

/// Prepare a question for display, translating every field ZH→EN with
/// per-field fallback to the original text.
pub async fn display_question(translator: &AnyTranslator, q: &AskQuestion) -> DisplayQuestion {
    let mut options = Vec::with_capacity(q.options.len());
    for o in &q.options {
        options.push(DisplayOption {
            label: display_field(translator, &o.label).await,
            description: display_field(translator, &o.description).await,
        });
    }
    DisplayQuestion {
        question: display_field(translator, &q.question).await,
        header: display_field(translator, &q.header).await,
        options,
        multi_select: q.multi_select,
    }
}

/// Resolve a picker outcome into the answer string the model expects.
///
/// `Ok(None)` means the user declined. Free text that fails EN→ZH
/// translation is an error — the outbound direction must not degrade
/// silently.
pub async fn resolve_outcome(
    translator: &AnyTranslator,
    q: &AskQuestion,
    outcome: PickOutcome,
) -> anyhow::Result<Option<String>> {
    match outcome {
        PickOutcome::Declined => Ok(None),
        PickOutcome::Picked(idxs) => {
            let labels: Vec<&str> = idxs
                .iter()
                .filter_map(|&i| q.options.get(i).map(|o| o.label.as_str()))
                .collect();
            Ok(Some(labels.join(", ")))
        }
        PickOutcome::FreeText(en) => {
            let sanitized = sanitize::sanitize(&en);
            let zh = translator
                .translate(&sanitized, Direction::EnToZh)
                .await
                .map_err(|e| anyhow::anyhow!("free-text answer EN→ZH translation failed: {e}"))?;
            Ok(Some(zh))
        }
    }
}

/// Handle one question request end-to-end with the given asker.
///
/// `ask` is synchronous (terminal interaction); injectable for tests.
async fn handle_request<F>(req: QuestionRequest, translator: AnyTranslator, ask: &F)
where
    F: Fn(DisplayQuestion) -> PickOutcome + Send + Sync,
{
    let mut answers = Vec::new();
    for q in &req.questions {
        let dq = display_question(&translator, q).await;
        let outcome = ask(dq);
        match resolve_outcome(&translator, q, outcome).await {
            Ok(Some(answer)) => answers.push(QuestionAnswer {
                question: q.question.clone(),
                answer,
            }),
            Ok(None) => {
                let _ = req.reply.send(QuestionReply::Decline);
                return;
            }
            Err(e) => {
                eprintln!("sigo: {e}; declining the question");
                let _ = req.reply.send(QuestionReply::Decline);
                return;
            }
        }
    }
    let _ = req.reply.send(QuestionReply::Answers(answers));
}

/// Run the bridge until the channel closes: for each request, suspend the
/// spinner, ask the user on the terminal, and reply with the resolved
/// answers.
pub async fn run_bridge(
    mut rx: mpsc::Receiver<QuestionRequest>,
    translator: SharedTranslator,
    spinner: SpinnerSlot,
) {
    let ask = move |dq: DisplayQuestion| -> PickOutcome {
        let pb = spinner.lock().unwrap().clone();
        tokio::task::block_in_place(|| {
            let do_pick = || {
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                let mut output = std::io::stdout();
                pick(&dq, &mut input, &mut output)
            };
            match pb {
                Some(pb) => pb.suspend(do_pick),
                None => do_pick(),
            }
        })
    };
    while let Some(req) = rx.recv().await {
        let translator = translator.lock().unwrap().clone();
        handle_request(req, translator, &ask).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sigo_core::{AskOption, FakeTranslator};
    use tokio::sync::oneshot;

    fn zh_question() -> AskQuestion {
        AskQuestion {
            question: "你最喜欢什么颜色？".into(),
            header: "颜色".into(),
            options: vec![
                AskOption {
                    label: "蓝色".into(),
                    description: "冷静".into(),
                },
                AskOption {
                    label: "红色".into(),
                    description: "热情".into(),
                },
            ],
            multi_select: false,
        }
    }

    #[tokio::test]
    async fn display_translates_every_field() {
        let fake = FakeTranslator::new_strict();
        fake.add_zh_to_en("你最喜欢什么颜色？", "What is your favorite color?");
        fake.add_zh_to_en("颜色", "Color");
        fake.add_zh_to_en("蓝色", "Blue");
        fake.add_zh_to_en("冷静", "Cool and calming");
        fake.add_zh_to_en("红色", "Red");
        fake.add_zh_to_en("热情", "Warm and energetic");
        let t = AnyTranslator::Fake(fake);

        let dq = display_question(&t, &zh_question()).await;
        assert_eq!(dq.question, "What is your favorite color?");
        assert_eq!(dq.header, "Color");
        assert_eq!(dq.options[0].label, "Blue");
        assert_eq!(dq.options[1].description, "Warm and energetic");
        assert!(!dq.multi_select);
    }

    #[tokio::test]
    async fn display_falls_back_to_raw_chinese_per_field_on_error() {
        let fake = FakeTranslator::new_strict();
        fake.add_zh_to_en("你最喜欢什么颜色？", "What is your favorite color?");
        // every other field is unmapped → strict fake errors → raw retained
        let t = AnyTranslator::Fake(fake);

        let dq = display_question(&t, &zh_question()).await;
        assert_eq!(dq.question, "What is your favorite color?");
        assert_eq!(dq.header, "颜色");
        assert_eq!(dq.options[0].label, "蓝色");
    }

    #[tokio::test]
    async fn picked_options_answer_with_original_untranslated_labels() {
        let t = AnyTranslator::Fake(FakeTranslator::new_strict());
        let q = zh_question();
        let one = resolve_outcome(&t, &q, PickOutcome::Picked(vec![1]))
            .await
            .unwrap();
        assert_eq!(one.as_deref(), Some("红色"));

        let mut multi = zh_question();
        multi.multi_select = true;
        let both = resolve_outcome(&t, &multi, PickOutcome::Picked(vec![0, 1]))
            .await
            .unwrap();
        assert_eq!(both.as_deref(), Some("蓝色, 红色"));
    }

    #[tokio::test]
    async fn free_text_is_sanitized_then_translated_en_to_zh() {
        let fake = FakeTranslator::new_strict();
        // The mapping is registered against the SANITIZED form — proving
        // sanitize runs before translation (control chars stripped).
        fake.add_en_to_zh("a darker shade of blue", "更深的蓝色");
        let t = AnyTranslator::Fake(fake);

        let got = resolve_outcome(
            &t,
            &zh_question(),
            PickOutcome::FreeText("a darker\u{0007} shade of blue".into()),
        )
        .await
        .unwrap();
        assert_eq!(got.as_deref(), Some("更深的蓝色"));
    }

    #[tokio::test]
    async fn free_text_translation_failure_is_an_error_not_silent() {
        let t = AnyTranslator::Fake(FakeTranslator::new_strict());
        let got =
            resolve_outcome(&t, &zh_question(), PickOutcome::FreeText("unmapped".into())).await;
        assert!(got.is_err(), "EN→ZH must not degrade silently");
    }

    #[tokio::test]
    async fn declined_outcome_is_none() {
        let t = AnyTranslator::Fake(FakeTranslator::new_strict());
        let got = resolve_outcome(&t, &zh_question(), PickOutcome::Declined)
            .await
            .unwrap();
        assert!(got.is_none());
    }

    #[tokio::test]
    async fn handle_request_answers_all_questions_with_original_text_keys() {
        let fake = FakeTranslator::new();
        let t = AnyTranslator::Fake(fake);
        let (reply_tx, reply_rx) = oneshot::channel();
        let req = QuestionRequest {
            questions: vec![zh_question()],
            reply: reply_tx,
        };

        handle_request(req, t, &|_dq| PickOutcome::Picked(vec![0])).await;

        match reply_rx.await.unwrap() {
            QuestionReply::Answers(answers) => {
                assert_eq!(answers.len(), 1);
                // the key is the ORIGINAL Chinese question, byte-for-byte
                assert_eq!(answers[0].question, "你最喜欢什么颜色？");
                assert_eq!(answers[0].answer, "蓝色");
            }
            other => panic!("expected answers, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_request_decline_propagates() {
        let t = AnyTranslator::Fake(FakeTranslator::new());
        let (reply_tx, reply_rx) = oneshot::channel();
        let req = QuestionRequest {
            questions: vec![zh_question()],
            reply: reply_tx,
        };

        handle_request(req, t, &|_dq| PickOutcome::Declined).await;

        assert!(matches!(reply_rx.await.unwrap(), QuestionReply::Decline));
    }
}
