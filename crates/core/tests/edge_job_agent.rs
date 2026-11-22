//! A job logs in with an agent profile (#127, found untested by the review
//! of #112): the profile's agent is the system's, the one `SSH_AUTH_SOCK`
//! names, set here (for this process only) to an in-process test agent so
//! that the user's own agent is never reached. It is the one test of this
//! binary: changing the environment while other tests run would be a
//! race (as `system_agent.rs`).

mod support;

use std::time::Duration;

use enodix_core::credentials::{CredentialProfile, ProfileAuth};
use enodix_core::job::{HostState, JobOptions};
use support::agent::TestAgent;
use support::jobs::{folder, host, run, snippet, Lab};
use support::{TestSshServer, USERNAME};

#[tokio::test]
async fn a_job_logs_in_with_an_agent_profile() {
    let device = TestSshServer::start_device("r1").await;
    let agent = TestAgent::start(&[device.client_key().clone()]).await;
    std::env::set_var("SSH_AUTH_SOCK", agent.path());
    let lab = Lab::new(&[
        ("_snippets.yaml", "Show: show hostname\n".into()),
        (
            "r1.yaml",
            host(&device, "frr", "credential_profile: agent\n"),
        ),
    ]);
    lab.put(CredentialProfile {
        name: "agent".into(),
        username: USERNAME.into(),
        auth: ProfileAuth::Agent,
        enable_secret: None,
    });
    let plan = lab.plan(&folder(""), &snippet("Show"));
    let ran = run(
        &support::manager(),
        plan,
        JobOptions::default(),
        Duration::from_secs(30),
    )
    .await;
    let r1 = ran.result("r1.yaml");
    assert_eq!(r1.state, HostState::Done, "{}", r1.output);
    assert!(r1.output.contains("\nr1\n"), "{}", r1.output);
}
