// SPDX-License-Identifier: Apache-2.0
// Explicit compatibility errors for the removed VM-template commands.
use anyhow::{bail, Result};
pub async fn init_factory_command() -> Result<()> {
    bail!("kata-fc-minimal: VM templates are unsupported")
}
pub async fn destroy_factory_command() -> Result<()> {
    bail!("kata-fc-minimal: VM templates are unsupported")
}
pub async fn status_factory_command() -> Result<()> {
    bail!("kata-fc-minimal: VM templates are unsupported")
}
