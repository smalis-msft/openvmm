// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Check every OpenVMM workspace feature with `cargo-hack`.

use flowey::node::prelude::*;

flowey_request! {
    pub struct Request {
        pub done: WriteVar<SideEffect>,
    }
}

new_simple_flow_node!(struct Node);

impl SimpleFlowNode for Node {
    type Request = Request;

    fn imports(ctx: &mut ImportCtx<'_>) {
        ctx.import::<crate::git_checkout_openvmm_repo::Node>();
        ctx.import::<crate::install_openvmm_rust_build_essential::Node>();
        ctx.import::<flowey_lib_common::download_cargo_hack::Node>();
        ctx.import::<flowey_lib_common::install_dist_pkg::Node>();
        ctx.import::<flowey_lib_common::install_rust::Node>();
    }

    fn process_request(request: Self::Request, ctx: &mut NodeCtx<'_>) -> anyhow::Result<()> {
        let Request { done } = request;

        let mut deps = vec![
            ctx.reqv(crate::install_openvmm_rust_build_essential::Request),
            ctx.reqv(flowey_lib_common::download_cargo_hack::Request::InstallWithCargo),
        ];

        if matches!(
            ctx.platform(),
            FlowPlatform::Linux(FlowPlatformLinuxDistro::Ubuntu)
        ) {
            deps.push(ctx.reqv(
                |done| flowey_lib_common::install_dist_pkg::Request::Install {
                    package_names: vec!["libssl-dev".into(), "pkg-config".into()],
                    done,
                },
            ));
        }

        let openvmm_repo_path = ctx.reqv(crate::git_checkout_openvmm_repo::req::GetRepoDir);
        let rust_toolchain = ctx.reqv(flowey_lib_common::install_rust::Request::GetRustupToolchain);

        ctx.emit_rust_step("run cargo hack", |ctx| {
            done.claim(ctx);
            deps.claim(ctx);
            let openvmm_repo_path = openvmm_repo_path.claim(ctx);
            let rust_toolchain = rust_toolchain.claim(ctx);
            move |rt| {
                let openvmm_repo_path = rt.read(openvmm_repo_path);
                rt.sh.change_dir(openvmm_repo_path);

                let rust_toolchain = rt
                    .read(rust_toolchain)
                    .as_ref()
                    .map(|toolchain| format!("+{toolchain}"));
                flowey::shell_cmd!(
                    rt,
                    "cargo {rust_toolchain...}
                        hack
                        --workspace
                        --each-feature
                        --locked
                        --keep-going
                        --exclude crypto
                        check
                    "
                )
                .run()?;

                Ok(())
            }
        });

        Ok(())
    }
}
