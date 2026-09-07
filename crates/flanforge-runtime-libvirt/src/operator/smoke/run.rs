use std::time::Duration;

use flanforge_core::{Config, GuestChannelKind, Profile, VmName};
use flanforge_store::StateMutationLock;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    RuntimeError,
    actor::{CreateRequest, DefineRequest, LibvirtActor},
    agent::{GUEST_CONTRACT_VERSION, ensure_provisioned},
    checkpoint::{allocation_path as checkpoint_path, remove as remove_checkpoint},
    context::{
        actor_config, backend, ensure_lock, read_operator_public_key, read_privileged_public_key,
    },
    image::{GuestContract, load_published},
    manifest::{ServiceInstance, intent_for},
    seed::build_seed,
};

use super::{
    bind::bind_guest,
    checks::{
        check_guest, ensure_idle, ensure_not_cancelled, remaining, wait_for_address,
        wait_guest_ready,
    },
    cleanup::{finish, recover_and_cleanup},
    state,
};
use crate::operator::SmokeOutcome;

/// Boots, checks, and exactly removes one disposable VM without Forgejo.
///
/// The caller must hold the shared runtime mutation lock. Cancellation is
/// observed only between completed mutations, and cleanup is never cancelled.
///
/// # Errors
/// Returns an error when validation, lifecycle work, guest checks, or exact
/// cleanup fails.
pub async fn smoke_disposable(
    config: &Config,
    lock: &StateMutationLock,
    profile: &Profile,
    cancellation: &CancellationToken,
) -> Result<SmokeOutcome, RuntimeError> {
    ensure_not_cancelled(cancellation)?;
    let image_manifest_dir = backend(config)?.image_manifest_dir.clone();
    ensure_lock(config, lock)?;
    let operator_key = read_operator_public_key(config).await?;
    let privileged_key = read_privileged_public_key(config).await?;
    let state_dir = config.runtime.state_dir.clone();
    let instance_state = state_dir.clone();
    let instance =
        tokio::task::spawn_blocking(move || ServiceInstance::load_or_create(&instance_state))
            .await
            .map_err(|_| RuntimeError::manifest("service-instance task failed"))??;
    let actor = LibvirtActor::open(actor_config(config, instance.id())?).await?;
    let cleanup_timeout = Duration::from_secs(profile.cleanup_timeout_seconds);
    recover_and_cleanup(&state_dir, &actor, &instance, cleanup_timeout).await?;
    ensure_not_cancelled(cancellation)?;
    ensure_idle(&actor.inventory(Duration::from_secs(15)).await?)?;

    let template = profile.template.clone();
    let base = tokio::task::spawn_blocking(move || load_published(&image_manifest_dir, &template))
        .await
        .map_err(|_| RuntimeError::manifest("published-base task failed"))??;
    let contract = GuestContract::read(&base);
    contract
        .ensure_channel_supported(config.guest.channel, &config.guest.runner_user)
        .map_err(RuntimeError::guest)?;
    let source = base.pointer().map_err(RuntimeError::manifest)?;
    actor
        .check_source(source.clone(), Duration::from_secs(15))
        .await?;

    let allocation_id = Uuid::new_v4();
    let vm_name = VmName::new(format!(
        "{}smoke-{allocation_id}",
        config.runtime.vm_prefix.as_str()
    ))
    .map_err(RuntimeError::manifest)?;
    let seed_id = allocation_id.to_string();
    let hostname = vm_name.to_string();
    let guest_user = config.guest.runner_user.clone();
    let privileged_user = config.guest.privileged_user.clone();
    let seed = tokio::task::spawn_blocking(move || {
        build_seed(
            &seed_id,
            &hostname,
            operator_key.as_deref(),
            &guest_user,
            &privileged_user,
            privileged_key.as_deref(),
        )
    })
    .await
    .map_err(|_| RuntimeError::seed("seed creation task failed"))??;
    let manifest = intent_for(
        allocation_id,
        &vm_name,
        &instance,
        &state_dir,
        seed.host_key_alias,
    )?;
    ensure_not_cancelled(cancellation)?;
    let prepared = state::prepare(&state_dir, &manifest, &seed.known_hosts).await;
    let operation = match prepared {
        Ok(()) => {
            run_guest(
                config,
                profile,
                &actor,
                source,
                manifest,
                seed.image,
                vm_name,
                &contract,
                cancellation,
            )
            .await
        }
        Err(error) => Err(error),
    };
    let operation = if operation.is_ok() && cancellation.is_cancelled() {
        Err(RuntimeError::Cancelled)
    } else {
        operation
    };
    let cleanup = recover_and_cleanup(&state_dir, &actor, &instance, cleanup_timeout).await;
    finish(operation, cleanup)
}

#[allow(clippy::too_many_arguments)]
async fn run_guest(
    config: &Config,
    profile: &Profile,
    actor: &LibvirtActor,
    source: flanforge_libvirt_wire::VolumePointer,
    manifest: flanforge_libvirt_wire::OwnershipManifest,
    seed: Vec<u8>,
    vm_name: VmName,
    contract: &GuestContract,
    cancellation: &CancellationToken,
) -> Result<SmokeOutcome, RuntimeError> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(profile.boot_timeout_seconds);
    let storage_bytes =
        profile
            .storage_mb
            .checked_mul(1_048_576)
            .ok_or_else(|| RuntimeError::Configuration {
                message: "guest storage size overflows bytes".to_owned(),
            })?;
    ensure_not_cancelled(cancellation)?;
    let manifest = actor
        .create(
            CreateRequest {
                manifest,
                source,
                seed,
                storage_bytes,
            },
            remaining(deadline)?,
        )
        .await?;
    state::persist(&config.runtime.state_dir, &manifest).await?;
    remove_checkpoint(&checkpoint_path(
        &config.runtime.state_dir,
        manifest.allocation_id(),
    ))?;

    ensure_not_cancelled(cancellation)?;
    actor
        .define(
            DefineRequest {
                manifest: manifest.clone(),
                cpu_count: profile.cpu_count,
                memory_mb: profile.memory_mb,
            },
            remaining(deadline)?,
        )
        .await?;
    ensure_not_cancelled(cancellation)?;
    ensure_idle(&actor.inventory(remaining(deadline)?).await?)?;
    ensure_not_cancelled(cancellation)?;
    actor.start(manifest.clone(), remaining(deadline)?).await?;
    ensure_not_cancelled(cancellation)?;
    // The agent channel exists because the daemon may have no route to the
    // guest, so the smoke never waits for one either.
    let address = match config.guest.channel {
        GuestChannelKind::Ssh => Some(
            wait_for_address(
                actor,
                &manifest,
                cancellation,
                deadline,
                Duration::from_secs(config.runtime.poll_seconds),
            )
            .await?,
        ),
        GuestChannelKind::Agent => None,
    };
    let (control, session) = bind_guest(config, actor, &manifest, address, contract)?;
    wait_guest_ready(&control, &session, cancellation, deadline).await?;
    // Reachability is not provisioning: the baked helper names the gate that
    // failed instead of leaving the contract script to time out.
    if contract.is_readiness_gate_baked() {
        ensure_provisioned(
            control.channel(),
            &session,
            &config.guest.runner_user,
            GUEST_CONTRACT_VERSION,
            deadline,
            Duration::from_secs(config.runtime.poll_seconds),
        )
        .await
        .map_err(RuntimeError::guest)?;
    }
    check_guest(&control, &session, cancellation, deadline).await?;
    Ok(SmokeOutcome::new(vm_name, address))
}
