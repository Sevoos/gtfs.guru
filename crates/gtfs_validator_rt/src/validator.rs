use crate::context::RtSnapshotContext;
use gtfs_guru_core::{NoticeContainer, NoticeSeverity, ValidationNotice};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = panic.downcast_ref::<&str>() {
        (*text).to_string()
    } else if let Some(text) = panic.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

pub trait RtValidator: Send + Sync {
    fn name(&self) -> &'static str;
    fn validate(&self, context: &RtSnapshotContext<'_>, notices: &mut NoticeContainer);
}

#[derive(Default)]
pub struct RtValidatorRunner {
    validators: Vec<Box<dyn RtValidator>>,
}

impl RtValidatorRunner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<V: RtValidator + 'static>(&mut self, validator: V) {
        self.validators.push(Box::new(validator));
    }

    /// Sequential and in registration order. GTF-11 defers concurrency until
    /// profiling justifies it, and notice order is part of the report contract.
    pub fn run(&self, context: &RtSnapshotContext<'_>, notices: &mut NoticeContainer) {
        for validator in &self.validators {
            let result = catch_unwind(AssertUnwindSafe(|| validator.validate(context, notices)));
            if let Err(panic) = result {
                let mut notice = ValidationNotice::new(
                    "runtime_exception_in_rt_validator_error",
                    NoticeSeverity::Error,
                    format!("RT validator {} panicked", validator.name()),
                );
                notice.insert_context_field("validator", validator.name());
                notice.insert_context_field("message", panic_message(&*panic));
                notice.field_order = vec!["validator".into(), "message".into()];
                notices.push(notice);
            }
        }
    }
}

pub fn default_rt_runner() -> RtValidatorRunner {
    let mut runner = RtValidatorRunner::new();
    runner.register(crate::rules::header::HeaderValidator);
    runner
}
