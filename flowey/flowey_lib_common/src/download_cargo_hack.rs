// Copyright (c) Microsoft Corporation.
// Licensed under the MIT License.

//! Download (and install) a copy of `cargo-hack`.

use crate::cache::CacheHit;
use flowey::node::prelude::*;

flowey_config! {
    /// Config for the download_cargo_hack node.
    pub struct Config {
        /// Version of `cargo hack` to install (e.g: "0.6.45")
        pub version: Option<String>,
    }
}

flowey_request! {
    pub enum Request {
        /// Install `cargo-hack` as a `cargo` extension (invoked via `cargo hack`).
        InstallWithCargo(WriteVar<SideEffect>),
    }
}

new_flow_node_with_config!(struct Node);

impl FlowNodeWithConfig for Node {
    type Request = Request;
    type Config = Config;

    fn imports(ctx: &mut ImportCtx<'_>) {
        ctx.import::<crate::cache::Node>();
        ctx.import::<crate::cfg_persistent_dir_cargo_install::Node>();
        ctx.import::<crate::install_rust::Node>();
    }

    fn emit(
        config: Config,
        requests: Vec<Self::Request>,
        ctx: &mut NodeCtx<'_>,
    ) -> anyhow::Result<()> {
        let mut install_with_cargo = Vec::new();

        for req in requests {
            match req {
                Request::InstallWithCargo(v) => install_with_cargo.push(v),
            }
        }

        let version = config
            .version
            .ok_or(anyhow::anyhow!("missing config: version"))?;

        if install_with_cargo.is_empty() {
            return Ok(());
        }

        let cargo_hack_bin = ctx.platform().binary("cargo-hack");

        let cache_dir = ctx.emit_rust_stepv("create cargo-hack cache dir", |_| {
            |_| Ok(std::env::current_dir()?.absolute()?)
        });

        let cache_key = ReadVar::from_static(format!("cargo-hack-{version}"));
        let hitvar = ctx.reqv(|v| crate::cache::Request {
            label: "cargo-hack".into(),
            dir: cache_dir.clone(),
            key: cache_key,
            restore_keys: None,
            hitvar: v,
        });

        let cargo_install_persistent_dir =
            ctx.reqv(crate::cfg_persistent_dir_cargo_install::Request);
        let rust_toolchain = ctx.reqv(crate::install_rust::Request::GetRustupToolchain);
        let cargo_home = ctx.reqv(crate::install_rust::Request::GetCargoHome);

        ctx.emit_rust_step("installing cargo-hack", |ctx| {
            install_with_cargo.claim(ctx);

            let cache_dir = cache_dir.claim(ctx);
            let hitvar = hitvar.claim(ctx);
            let cargo_install_persistent_dir = cargo_install_persistent_dir.claim(ctx);
            let rust_toolchain = rust_toolchain.claim(ctx);
            let cargo_home = cargo_home.claim(ctx);

            move |rt| {
                let cache_dir = rt.read(cache_dir);

                let cached_bin_path = cache_dir.join(&cargo_hack_bin);
                let cached = if matches!(rt.read(hitvar), CacheHit::Hit) {
                    assert!(cached_bin_path.exists());
                    Some(cached_bin_path.clone())
                } else {
                    None
                };

                let path_to_cargo_hack = if let Some(cached) = cached {
                    cached
                } else {
                    let root = rt.read(cargo_install_persistent_dir).unwrap_or("./".into());

                    let rust_toolchain = rt.read(rust_toolchain);
                    let run = |offline| {
                        let rust_toolchain = rust_toolchain.as_ref().map(|s| format!("+{s}"));

                        flowey::shell_cmd!(
                            rt,
                            "cargo {rust_toolchain...}
                                install
                                --locked
                                {offline...}
                                --root {root}
                                --target-dir {root}
                                --version {version}
                                cargo-hack
                            "
                        )
                        .run()
                    };

                    if run(Some("--offline")).is_err() {
                        run(None)?;
                    }

                    let out_bin = root.absolute()?.join("bin").join(&cargo_hack_bin);
                    fs_err::rename(out_bin, &cached_bin_path)?;
                    cached_bin_path.absolute()?
                };

                fs_err::copy(
                    &path_to_cargo_hack,
                    rt.read(cargo_home).join("bin").join(&cargo_hack_bin),
                )?;

                Ok(())
            }
        });

        Ok(())
    }
}
