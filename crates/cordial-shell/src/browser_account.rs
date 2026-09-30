//! Resolve a browser's account without replacing a saved profile's session.

mod profile;
mod ticket;
mod transport;

use std::path::Path;
use std::sync::OnceLock;

use cordial_shell::secrets::Store;
pub(crate) use profile::ProfileMatch;
pub use ticket::LaunchTicket;
use transport::AccountId;

/// An unreadable profile cannot establish an account match. Duplicate accounts
/// deliberately leave the choice to the user instead of choosing by directory order.
pub(crate) fn matching_profile(
    root: &Path,
    account: AccountId,
    store: Store,
) -> Option<ProfileMatch> {
    profile::matching_profile_with(
        profile::ProfileQuery {
            root,
            account,
            store,
        },
        transport::authenticated,
    )
}

pub(crate) fn resolve(ticket: LaunchTicket) -> Option<ProfileMatch> {
    let account = transport::lookup(&ticket)?;
    let root = cordial_shell::profile::root();
    static STORE: OnceLock<Store> = OnceLock::new();
    let store = *STORE.get_or_init(|| cordial_shell::secrets::configured(&root.join("*/cookies")));
    matching_profile(&root, account, store)
}

#[cfg(test)]
mod keyring_tests;
#[cfg(test)]
mod tests;
