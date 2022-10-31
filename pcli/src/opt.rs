use crate::{
    box_grpc_svc::{self, BoxGrpcService},
    command::CommandRoot,
    legacy, warning, App,
};
use anyhow::{Context, Result};
use camino::Utf8PathBuf;
use clap::Parser;
use directories::ProjectDirs;
use penumbra_crypto::FullViewingKey;
use penumbra_custody::SoftHSM;
use penumbra_proto::{
    custody::v1alpha1::{
        custody_protocol_client::CustodyProtocolClient,
        custody_protocol_server::CustodyProtocolServer,
    },
    view::v1alpha1::{
        view_protocol_client::ViewProtocolClient, view_protocol_server::ViewProtocolServer,
    },
};
use penumbra_view::ViewService;
use penumbra_wallet::KeyStore;
use std::{fs, net::SocketAddr};
use tracing_subscriber::EnvFilter;
use url::{Host, Url};

#[derive(Debug, Parser)]
#[clap(
    name = "pcli",
    about = "The Penumbra command-line interface.",
    version = env!("VERGEN_GIT_SEMVER"),
)]
pub struct Opt {
    /// The hostname of the pd+tendermint node.
    #[clap(
        short,
        long,
        default_value = "testnet.penumbra.zone",
        env = "PENUMBRA_NODE_HOSTNAME",
        parse(try_from_str = url::Host::parse)
    )]
    node: url::Host,
    /// The port to use to speak to tendermint's RPC server.
    #[clap(long, default_value_t = 26657, env = "PENUMBRA_TENDERMINT_PORT")]
    tendermint_port: u16,
    /// The port to use to speak to pd's gRPC server.
    #[clap(long, default_value_t = 8080, env = "PENUMBRA_PD_PORT")]
    pd_port: u16,
    #[clap(subcommand)]
    pub cmd: CommandRoot,
    /// The directory to store the wallet and view data in.
    #[clap(short, long, default_value_t = default_data_dir())]
    pub data_path: Utf8PathBuf,
    /// If set, use a remote view service instead of local synchronization.
    #[clap(short, long, env = "PENUMBRA_VIEW_ADDRESS")]
    view_address: Option<SocketAddr>,
    /// The filter for `pcli`'s log messages.
    #[clap( long, default_value_t = EnvFilter::new("warn"), env = "RUST_LOG")]
    trace_filter: EnvFilter,
}

impl Opt {
    pub fn init_tracing(&mut self) {
        tracing_subscriber::fmt()
            .with_env_filter(std::mem::take(&mut self.trace_filter))
            .init();
    }

    pub fn into_init_app(mut self) -> Result<(InitApp, CommandRoot)> {
        // Display a warning message to the user so they don't get upset when all their tokens are lost.
        if std::env::var("PCLI_UNLEASH_DANGER").is_err() {
            warning::display();
        }

        // Initialize tracing here, rather than when converting into an `App`, so
        // that tracing is set up even for wallet commands that don't build the `App`.
        self.init_tracing();

        //Ensure that the data_path exists, in case this is a cold start
        fs::create_dir_all(&self.data_path)
            .with_context(|| format!("Failed to create data directory {}", self.data_path))?;

        let custody_path = self.data_path.join(crate::CUSTODY_FILE_NAME);
        let legacy_wallet_path = self.data_path.join(legacy::WALLET_FILE_NAME);

        // Try to auto-migrate the legacy wallet file to the new location, if:
        // - the legacy wallet file exists
        // - the new wallet file does not exist
        if legacy_wallet_path.exists() && !custody_path.exists() {
            legacy::migrate(&legacy_wallet_path, &custody_path.as_path())?;
        }

        Ok((
            InitApp {
                custody_path,
                data_path: self.data_path,
                view_address: self.view_address,
                pd_port: self.pd_port,
                tendermint_port: self.tendermint_port,
                node: self.node,
            },
            self.cmd,
        ))
    }
}

pub struct InitApp {
    pub data_path: Utf8PathBuf,
    custody_path: Utf8PathBuf,
    view_address: Option<SocketAddr>,
    node: Host<String>,
    pd_port: u16,
    tendermint_port: u16,
}

impl InitApp {
    pub fn into_offline_app(self) -> Result<OfflineApp> {
        // Build the custody service...
        let wallet = KeyStore::load(self.custody_path)?;
        let soft_hsm = SoftHSM::new(vec![wallet.spend_key.clone()]);
        let custody_svc = CustodyProtocolServer::new(soft_hsm);
        let custody = CustodyProtocolClient::new(box_grpc_svc::local(custody_svc));

        let fvk = wallet.spend_key.full_viewing_key().clone();

        Ok(OfflineApp {
            custody,
            fvk,
            wallet,
            node: self.node,
            data_path: self.data_path,
            view_address: self.view_address,
            pd_port: self.pd_port,
            tendermint_port: self.tendermint_port,
        })
    }
}

pub struct OfflineApp {
    data_path: Utf8PathBuf,
    custody: CustodyProtocolClient<BoxGrpcService>,
    fvk: FullViewingKey,
    wallet: KeyStore,
    view_address: Option<SocketAddr>,
    node: Host,
    pd_port: u16,
    tendermint_port: u16,
}

impl OfflineApp {
    pub async fn into_app(self) -> Result<App> {
        // Parse urls
        let mut tendermint_url = format!("http://{}", self.node)
            .parse::<Url>()
            .with_context(|| format!("Invalid node URL: {}", self.node))?;
        let mut pd_url = tendermint_url.clone();

        pd_url
            .set_port(Some(self.pd_port))
            .expect("pd URL will not be `file://`");
        tendermint_url
            .set_port(Some(self.tendermint_port))
            .expect("tendermint URL will not be `file://`");

        let mut app = App {
            pd_url,
            tendermint_url,
            view: self.view_client(&self.fvk).await?,
            custody: self.custody,
            fvk: self.fvk,
            wallet: self.wallet,
        };

        app.sync();

        Ok(app)
    }

    /// Constructs a [`ViewProtocolClient`] based on the command-line options.
    async fn view_client(
        &self,
        fvk: &FullViewingKey,
    ) -> Result<ViewProtocolClient<BoxGrpcService>> {
        let svc = if let Some(address) = self.view_address {
            // Use a remote view service.
            tracing::info!(%address, "using remote view service");

            let ep = tonic::transport::Endpoint::new(format!("http://{}", address))?;
            box_grpc_svc::connect(ep).await?
        } else {
            // Use an in-memory view service.
            let path = self.data_path.join(crate::VIEW_FILE_NAME);
            tracing::info!(%path, "using local view service");

            let svc = ViewService::load_or_initialize(
                path,
                fvk,
                self.node.to_string(),
                self.pd_port,
                self.tendermint_port,
            )
            .await?;

            // Now build the view and custody clients, doing gRPC with ourselves
            let svc = ViewProtocolServer::new(svc);
            box_grpc_svc::local(svc)
        };

        Ok(ViewProtocolClient::new(svc))
    }
}

fn default_data_dir() -> Utf8PathBuf {
    let path = ProjectDirs::from("zone", "penumbra", "pcli")
        .expect("Failed to get platform data dir")
        .data_dir()
        .to_path_buf();
    Utf8PathBuf::from_path_buf(path).expect("Platform default data dir was not UTF-8")
}
