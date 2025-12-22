use std::io::Result;
use std::path::PathBuf;

fn main() -> Result<()> {
    let proto_root = PathBuf::from("../../api/proto");

    let protos = [
        "plfm/common/v1/ids.proto",
        "plfm/common/v1/errors.proto",
        "plfm/controlplane/v1/org.proto",
        "plfm/controlplane/v1/project.proto",
        "plfm/controlplane/v1/app.proto",
        "plfm/controlplane/v1/env.proto",
        "plfm/events/v1/envelope.proto",
        "plfm/events/v1/org.proto",
        "plfm/events/v1/project.proto",
        "plfm/events/v1/app.proto",
        "plfm/events/v1/env.proto",
        "plfm/events/v1/release.proto",
        "plfm/events/v1/deploy.proto",
        "plfm/events/v1/route.proto",
        "plfm/events/v1/secret.proto",
        "plfm/events/v1/volume.proto",
        "plfm/events/v1/instance.proto",
        "plfm/events/v1/node.proto",
        "plfm/events/v1/exec.proto",
        "plfm/agent/v1/workload.proto",
        "plfm/agent/v1/agent.proto",
    ];

    let proto_paths: Vec<PathBuf> = protos.iter().map(|p| proto_root.join(p)).collect();

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .out_dir("src/gen")
        .file_descriptor_set_path("src/gen/plfm_descriptor.bin")
        .compile_protos(&proto_paths, &[&proto_root])?;

    for proto in &protos {
        println!(
            "cargo:rerun-if-changed={}",
            proto_root.join(proto).display()
        );
    }

    Ok(())
}
