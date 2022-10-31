// Rust analyzer complains without this (but rustc is happy regardless)
#![recursion_limit = "256"]
#![allow(clippy::clone_on_copy)]

use anyhow::Result;

use clap::Parser;
use futures::StreamExt;
use penumbra_crypto::FullViewingKey;

use penumbra_proto::{
    custody::v1alpha1::custody_protocol_client::CustodyProtocolClient,
    view::v1alpha1::view_protocol_client::ViewProtocolClient,
};
use penumbra_view::ViewClient;
use url::Url;

mod box_grpc_svc;
mod command;
mod legacy;
mod network;
mod opt;
mod warning;

use opt::Opt;
use penumbra_wallet::KeyStore;

use box_grpc_svc::BoxGrpcService;
use command::*;

const CUSTODY_FILE_NAME: &str = "custody.json";
const VIEW_FILE_NAME: &str = "pcli-view.sqlite";

#[derive(Debug)]
pub struct App {
    pub view: ViewProtocolClient<BoxGrpcService>,
    pub custody: CustodyProtocolClient<BoxGrpcService>,
    pub fvk: FullViewingKey,
    pub wallet: KeyStore,
    pub pd_url: Url,
    pub tendermint_url: Url,
}

impl App {
    pub fn view(&mut self) -> &mut impl ViewClient {
        &mut self.view
    }

    async fn sync(&mut self) -> Result<()> {
        let mut status_stream = ViewClient::status_stream(&mut self.view, self.fvk.hash()).await?;

        // Pull out the first message from the stream, which has the current state, and use
        // it to set up a progress bar.
        let initial_status = status_stream
            .next()
            .await
            .transpose()?
            .ok_or_else(|| anyhow::anyhow!("view service did not report sync status"))?;

        println!(
            "Scanning blocks from last sync height {} to latest height {}",
            initial_status.sync_height, initial_status.latest_known_block_height,
        );

        use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
        let progress_bar = ProgressBar::with_draw_target(
            initial_status.latest_known_block_height - initial_status.sync_height,
            ProgressDrawTarget::stdout(),
        )
        .with_style(
            ProgressStyle::default_bar()
                .template("[{elapsed}] {bar:50.cyan/blue} {pos:>7}/{len:7} {per_sec} ETA: {eta}"),
        );
        progress_bar.set_position(0);

        while let Some(status) = status_stream.next().await.transpose()? {
            progress_bar.set_position(status.sync_height - initial_status.sync_height);
        }
        progress_bar.finish();

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let opt = Opt::parse();

    let (pre_init_app, cmd) = opt.into_init_app()?;

    if let CommandRoot::Init(init_commands) = &cmd {
        init_commands.exec(pre_init_app);
        return Ok(());
    }

    let offline_app = pre_init_app.into_offline_app()?;

    // Run offline_app commands

    let app = offline_app.into_app().await?;

    // match &cmd {
    //     Command::Keys(_) => unreachable!("wallet command already executed"),
    //     Command::Transaction(tx_cmd) => tx_cmd.exec(&mut app).await?,
    //     Command::View(view_cmd) => {
    //         let mut oblivious_client = app.oblivious_client().await?;

    //         view_cmd
    //             .exec(&app.fvk, Some(&mut app.view), &mut oblivious_client)
    //             .await?
    //     }
    //     Command::Validator(cmd) => cmd.exec(&mut app).await?,
    //     Command::Query(cmd) => cmd.exec(&mut app).await?,
    // }

    Ok(())
}
