// SPDX-License-Identifier: Apache-2.0
use std::fs;

#[test]
fn responses_transport_future_is_boxed_off_the_windows_worker_stack() {
    let source = fs::read_to_string("src/agent/model_transport.rs")
        .expect("read model transport source");
    assert!(source.contains("Box::pin(self.call_openai_responses_model("));
    assert!(source.contains("0xc00000fd"));
}
