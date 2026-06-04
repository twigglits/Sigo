mod lru_cache;
mod storage;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(ui::run())
}
