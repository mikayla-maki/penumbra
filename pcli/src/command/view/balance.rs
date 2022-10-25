use std::collections::BTreeMap;

use anyhow::Result;
use comfy_table::{presets, Table};
use penumbra_crypto::{asset::Cache, keys::AddressIndex, Amount, FullViewingKey, Value};
use penumbra_view::{QuarantinedNoteRecord, SpendableNoteRecord, ViewClient};
#[derive(Debug, clap::Args)]
pub struct BalanceCmd {
    /// If set, breaks down balances by address.
    #[clap(short, long)]
    pub by_address: bool,
    #[clap(long)]
    /// If set, prints the value of each note individually.
    pub by_note: bool,
}

impl BalanceCmd {
    pub fn offline(&self) -> bool {
        false
    }

    // Big issue with this code is that we're doing a lot of table level work inside Rust. Cleaner
    // solution: Add more queries to the view that cover each of these 4 branches?
    pub async fn exec<V: ViewClient>(&self, fvk: &FullViewingKey, view: &mut V) -> Result<()> {
        let asset_cache = view.assets().await?;

        // Initialize the table
        let mut table = Table::new();
        table.load_preset(presets::NOTHING);

        // `Option<u64>` indicates the unbonding epoch, if any, for a quarantined note
        let rows: Vec<(Option<AddressIndex>, Value, Option<u64>)> = if self.by_address {
            let notes = view.unspent_notes_by_address_and_asset(fvk.hash()).await?;
            let quarantined_notes = view
                .quarantined_notes_by_address_and_asset(fvk.hash())
                .await?;

            if self.by_note {
                // When using --by-note, just reflect the data as found
                collect_notes(
                    notes,
                    quarantined_notes,
                    |index, _| Some(*index),
                    |_, asset, amount| asset.value(amount),
                )
            } else {
                // When using just --by-address, we need to group by addresses *and* assets
                notes
                    .iter()
                    .flat_map(|(index, notes_by_asset)| {
                        // Sum the notes for each asset:
                        notes_by_asset.iter().map(|(asset, notes)| {
                            let sum: u64 = notes
                                .iter()
                                .map(|record| u64::from(record.note.amount()))
                                .sum();
                            (Some(*index), asset.value(sum.into()), None)
                        })
                    })
                    .chain(
                        quarantined_notes
                            .iter()
                            .flat_map(|(index, notes_by_asset)| {
                                // Sum the notes for each asset, separating them by unbonding epoch:
                                notes_by_asset.iter().flat_map(|(asset, notes)| {
                                    let mut sums_by_unbonding_epoch = BTreeMap::<u64, u64>::new();
                                    for record in notes {
                                        let unbonding_epoch = record.unbonding_epoch;
                                        *sums_by_unbonding_epoch
                                            .entry(unbonding_epoch)
                                            .or_default() += u64::from(record.note.amount());
                                    }
                                    sums_by_unbonding_epoch.into_iter().map(
                                        |(unbonding_epoch, sum)| {
                                            (
                                                Some(*index),
                                                asset.value(sum.into()),
                                                Some(unbonding_epoch),
                                            )
                                        },
                                    )
                                })
                            }),
                    )
                    .collect()
            }
        } else {
            let notes = view.unspent_notes_by_asset_and_address(fvk.hash()).await?;
            let quarantined_notes = view
                .quarantined_notes_by_asset_and_address(fvk.hash())
                .await?;

            if self.by_note {
                // When using --by-note, just reflect the data as found
                collect_notes(
                    notes,
                    quarantined_notes,
                    |_, index| Some(*index),
                    |asset, _, amount| asset.value(amount),
                )
            } else {
                // When using neither --by-address, nor --by-note, we need to collapse adresses, *but retain* the assets grouping
                notes
                    .iter()
                    .map(|(asset, notes_by_index)| {
                        //Asset
                        // Sum the notes for each asset:
                        let sum: u64 = notes_by_index
                            .values()
                            .flat_map(|notes| {
                                //Index
                                notes.iter().map(|record| u64::from(record.note.amount()))
                            })
                            .sum();
                        (None, asset.value(sum.into()), None)
                    })
                    .chain(
                        quarantined_notes
                            .iter()
                            .flat_map(|(asset, notes_by_index)| {
                                // Sum the notes for each asset, separating them by unbonding epoch:
                                let mut sums_by_unbonding_epoch = BTreeMap::<u64, u64>::new();
                                for records in notes_by_index.values() {
                                    for record in records {
                                        let unbonding_epoch = record.unbonding_epoch;
                                        *sums_by_unbonding_epoch
                                            .entry(unbonding_epoch)
                                            .or_default() += u64::from(record.note.amount());
                                    }
                                }
                                sums_by_unbonding_epoch
                                    .into_iter()
                                    .map(|(unbonding_epoch, sum)| {
                                        (None, asset.value(sum.into()), Some(unbonding_epoch))
                                    })
                            }),
                    )
                    .collect()
            }
        };

        let (indexed_rows, ephemeral_rows) = combine_ephemeral(rows, self.by_note);

        // Print table
        if self.by_address {
            table.set_header(vec!["Addr Index", "Amount"]);
        } else {
            table.set_header(vec!["Amount"]);
        }

        for (index, value, quarantined) in indexed_rows.iter().chain(ephemeral_rows.iter()) {
            let row = format_row(index, value, quarantined, self.by_address, &asset_cache);

            table.add_row(row);
        }

        println!("{}", table);

        Ok(())
    }
}

fn format_row(
    index: &Option<AddressIndex>,
    value: &Value,
    quarantined: &Option<u64>,
    by_address: bool,
    asset_cache: &Cache,
) -> Vec<String> {
    let mut string_row = Vec::with_capacity(2);

    if by_address {
        let index = u128::from(index.expect("--by-address specified, but no address set for note"));
        let index_text = if index < u64::MAX as u128 {
            format!("{}", index)
        } else {
            "Ephemeral".to_string()
        };

        string_row.push(index_text)
    }
    string_row.push(format!(
        "{}{}",
        value.format(&asset_cache),
        if let Some(unbonding_epoch) = quarantined {
            format!(" (unbonding until epoch {})", unbonding_epoch)
        } else {
            "".to_string()
        }
    ));

    string_row
}

fn combine_ephemeral(
    rows: Vec<(Option<AddressIndex>, Value, Option<u64>)>,
    by_note: bool,
) -> (
    Vec<(Option<AddressIndex>, Value, Option<u64>)>,
    Vec<(Option<AddressIndex>, Value, Option<u64>)>,
) {
    // get all ephemeral rows
    let (mut ephemeral_notes, indexed_rows): (Vec<_>, Vec<_>) =
        rows.into_iter().partition(|(index, _, _)| {
            if let Some(index) = index {
                u128::from(*index) > u64::MAX as u128
            } else {
                false
            }
        });

    // combine by asset ID
    let ephemeral_rows = if by_note || ephemeral_notes.len() <= 1 {
        ephemeral_notes
    } else {
        ephemeral_notes.sort_by(|row1, row2| row1.1.asset_id.cmp(&row2.1.asset_id));
        let mut new_ephemeral_notes = vec![];
        let mut cur_row = ephemeral_notes[0];
        for row in ephemeral_notes.iter().skip(1) {
            if cur_row.1.asset_id == row.1.asset_id {
                cur_row.1.amount = cur_row.1.amount + row.1.amount;
            } else {
                new_ephemeral_notes.push(cur_row);
                cur_row = *row;
            }
        }
        new_ephemeral_notes
    };
    (indexed_rows, ephemeral_rows)
}

// G1 / G2 mean 'group 1' and 'group 2'. Depending on the code path, these
// should be either `AddressIndex, Id` or `Id, AddressIndex`. Due to the
// indeterminate order, we have to use generics and we have to pass in the selector
// functions to disambiguate the two
fn collect_notes<G1, G2, F1, F2>(
    notes: BTreeMap<G1, BTreeMap<G2, Vec<SpendableNoteRecord>>>,
    quarantined_notes: BTreeMap<G1, BTreeMap<G2, Vec<QuarantinedNoteRecord>>>,
    index_selector: F1,
    value_selector: F2,
) -> Vec<(Option<AddressIndex>, Value, Option<u64>)>
where
    F1: Fn(&G1, &G2) -> Option<AddressIndex>,
    F2: Fn(&G1, &G2, Amount) -> Value,
{
    notes
        .iter()
        .flat_map(|(g1, notes_by_g1)| {
            // Include each note individually:
            notes_by_g1.iter().flat_map(|(g2, notes_by_g2)| {
                notes_by_g2.iter().map(|record| {
                    (
                        index_selector(g1, g2),
                        value_selector(g1, g2, record.note.amount()),
                        None,
                    )
                })
            })
        })
        .chain(quarantined_notes.iter().flat_map(|(g1, notes_by_g1)| {
            // Include each note individually:
            notes_by_g1.iter().flat_map(|(g2, notes_by_g2)| {
                notes_by_g2.iter().map(|record| {
                    (
                        index_selector(g1, g2),
                        value_selector(g1, g2, record.note.amount()),
                        Some(record.unbonding_epoch),
                    )
                })
            })
        }))
        .collect()
}
