mod keys;
mod query;
mod tx;
mod validator;
mod view;

use std::{fs::File, io::Write};

use anyhow::{Context, Result};
pub use keys::KeysCmd;
use penumbra_component::stake::{validator::Validator, FundingStream, FundingStreams};
use penumbra_crypto::{GovernanceKey, IdentityKey};
pub use query::QueryCmd;
pub use tx::TxCmd;
pub use validator::ValidatorCmd;
pub use view::transaction_hashes::TransactionHashesCmd;
pub use view::ViewCmd;

use crate::opt::{InitApp, OfflineApp};

// Note on display_order:
//
// The value is between 0 and 999 (the default).  Sorting of subcommands is done
// by display_order first, and then alphabetically.  We should not try to order
// every set of subcommands -- for instance, it doesn't make sense to try to
// impose a non-alphabetical ordering on the query subcommands -- but we can use
// the order to group related commands.
//
// Setting spaced numbers is future-proofing, letting us insert other commands
// without noisy renumberings.
//
// https://docs.rs/clap/latest/clap/builder/struct.App.html#method.display_order

#[derive(Debug, clap::Subcommand)]
pub enum CommandRoot {
    #[clap(flatten)]
    Init(InitCommands),
    #[clap(flatten)]
    Offline(OfflineCommands),
    #[clap(flatten)]
    Online(OnlineCommands),
}

#[derive(Debug, clap::Subcommand)]
pub enum InitCommands {
    #[clap(subcommand, display_order = 300, visible_alias = "v")]
    View(ViewCmd),
    #[clap(subcommand, display_order = 500)]
    Keys(KeysCmd),
}

#[derive(Debug, clap::Subcommand)]
pub enum OfflineCommands {
    #[clap(subcommand, display_order = 300, visible_alias = "v")]
    View(ViewCmd),
    /// Manage a validator.
    #[clap(subcommand, display_order = 998)]
    Validator(ValidatorCmd),
}

#[derive(Debug, clap::Subcommand)]
pub enum OnlineCommands {
    #[clap(subcommand, display_order = 200, visible_alias = "q")]
    Query(QueryCmd),
    /// View your private chain state, like account balances.
    #[clap(subcommand, display_order = 300, visible_alias = "v")]
    View(ViewCmd),
    #[clap(subcommand, display_order = 400, visible_alias = "tx")]
    Transaction(TxCmd),
    /// Manage a validator.
    #[clap(subcommand, display_order = 998)]
    Validator(ValidatorCmd),
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Query the public chain state, like the validator set.
    ///
    /// This command has two modes: it can be used to query raw bytes of
    /// arbitrary keys with the `key` subcommand, or it can be used to query
    /// typed data with a subcommand for a particular component.
    #[clap(subcommand, display_order = 200, visible_alias = "q")]
    Query(QueryCmd),
    /// View your private chain state, like account balances.
    #[clap(subcommand, display_order = 300, visible_alias = "v")]
    View(ViewCmd),
    /// Create and broadcast a transaction.
    #[clap(subcommand, display_order = 400, visible_alias = "tx")]
    Transaction(TxCmd),
    /// Manage your wallet's keys.
    #[clap(subcommand, display_order = 500)]
    Keys(KeysCmd),
    /// Manage a validator.
    #[clap(subcommand, display_order = 998)]
    Validator(ValidatorCmd),
}

impl Command {
    /// Determine if this command requires a network sync before it executes.
    pub fn offline(&self) -> bool {
        match self {
            Command::Transaction(cmd) => cmd.offline(),
            Command::View(cmd) => cmd.offline(),
            Command::Keys(cmd) => cmd.offline(),
            Command::Validator(cmd) => cmd.offline(),
            Command::Query(_) => false,
        }
    }
}

impl InitCommands {
    pub fn exec(&self, app: InitApp) -> Result<()> {
        match self {
            // The keys command takes the data dir directly, since it may need to
            // create the client state, so handle it specially here so that we can have
            // common code for the other subcommands.
            InitCommands::Keys(keys_cmd) => {
                keys_cmd.exec(app.data_path.as_path())?;
                Ok(())
            }
            // The view reset command takes the data dir directly, and should not be invoked when there's a
            // view service running.
            InitCommands::View(ViewCmd::Reset(reset)) => {
                reset.exec(app.data_path.as_path())?;
                Ok(())
            }
            _ => {
                unreachable!("This shouldn't happen... but probably will")
            }
        }
    }
}

impl OfflineCommands {
    pub async fn exec(&self, app: OfflineApp) -> Result<()> {
        match self {
            OfflineCommands::View(view_cmd) => match view_cmd {
                ViewCmd::Address(address_cmd) => {
                    address_cmd.exec(&app.fvk)?;
                    Ok(())
                }
                _ => {
                    unreachable!("This shouldn't happen... but probably will")
                }
            },
            OfflineCommands::Validator(validator_commands) => match validator_commands {
                ValidatorCmd::Identity => {
                    let ik = IdentityKey(app.fvk.spend_verification_key().clone());

                    println!("{}", ik);
                    Ok(())
                }
                ValidatorCmd::Definition(validator::DefinitionCmd::Template { file }) => {
                    let (address, _dtk) = app.fvk.incoming().payment_address(0u64.into());
                    let identity_key = IdentityKey(app.fvk.spend_verification_key().clone());
                    // By default, the template sets the governance key to the same verification key as
                    // the identity key, but a validator can change this if they want to use different
                    // key material.
                    let governance_key = GovernanceKey(identity_key.0);
                    // Generate a random consensus key.
                    // TODO: not great because the private key is discarded here and this isn't obvious to the user
                    let consensus_key =
                        tendermint::PrivateKey::Ed25519(ed25519_consensus::SigningKey::new(OsRng))
                            .public_key();

                    let template = Validator {
                        identity_key,
                        governance_key,
                        consensus_key,
                        name: String::new(),
                        website: String::new(),
                        description: String::new(),
                        // Default enabled to "false" so operators are required to manually
                        // enable their validators when ready.
                        enabled: false,
                        funding_streams: FundingStreams::try_from(vec![FundingStream {
                            address,
                            rate_bps: 100,
                        }])?,
                        sequence_number: 0,
                    };

                    if let Some(file) = file {
                        File::create(file)
                            .with_context(|| format!("cannot create file {:?}", file))?
                            .write_all(&serde_json::to_vec_pretty(&template)?)
                            .context("could not write file")?;
                    } else {
                        println!("{}", serde_json::to_string_pretty(&template)?);
                    }

                    Ok(())
                }
                ValidatorCmd::Definition(validator::DefinitionCmd::Fetch { file }) => {
                    let identity_key = IdentityKey(app.fvk.spend_verification_key().clone());

                    self::query::ValidatorCmd::Definition {
                        file: file.clone(),
                        identity_key: identity_key.to_string(),
                    }
                    .exec(app)
                    .await?;

                    Ok(())
                }
                _ => {
                    unreachable!("This shouldn't happen... but probably will")
                }
            },
        }
    }
}
