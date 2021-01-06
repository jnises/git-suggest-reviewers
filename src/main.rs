use anyhow::{Result, anyhow, Context};
use structopt::StructOpt;
use git2::{Repository, DiffFormat};

#[derive(Debug, StructOpt)]
#[structopt(about = "List authors of lines changed by PR")]
struct Opt {
    /// where to merge to
    #[structopt()]
    base: String,

    /// where to merge from
    #[structopt()]
    compare: String,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    let repo = Repository::open(".")?;
    let base = repo.find_reference(&opt.base).context("unable to find base")?.resolve()?;
    let base_oid = base.target().ok_or_else(|| anyhow!("cannot find base {:?}", base.name()))?;
    let compare = repo.find_reference(&opt.compare)?.resolve()?;
    let compare_oid = compare.target().ok_or_else(|| anyhow!("cannot find compare {:?}", base.name()))?;
    let merge_base = repo.merge_base(base_oid, compare_oid)?;
    let merge_base_tree = repo.find_tree(merge_base)?;
    println!("merge base: {:?}", merge_base_tree);
    let diff = repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&compare.peel_to_tree()?), None)?;
    diff.print(DiffFormat::Patch, |delta, hunk, line| {
        println!("{:?} {:?} {:?}", delta, hunk, line);
        true
    })?;
    Ok(())
}
