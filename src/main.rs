use anyhow::{anyhow, Context, Result};
use git2::{Blame, DiffDelta, DiffFormat, DiffHunk, DiffLine, Oid, Repository};
use log::{info, warn};
use std::path::PathBuf;
use structopt::StructOpt;

#[derive(Debug, StructOpt)]
#[structopt(about = "List authors of lines changed by PR")]
struct Opt {
    /// where to merge to
    base: String,

    /// where to merge from
    compare: String,

    #[structopt(short, long)]
    verbose: bool,
}

fn ref_or_id(repo: &Repository, name: &str) -> Result<Oid> {
    Ok(match Oid::from_str(name) {
        Ok(oid) => oid,
        Err(_) => repo.refname_to_id(name)?,
    })
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    env_logger::builder()
        .filter_level(if opt.verbose {
            log::LevelFilter::Info
        } else {
            log::LevelFilter::Error
        })
        .init();
    let repo = Repository::open(".")?;
    let base = ref_or_id(&repo, &opt.base).context("unable to find base")?;
    let compare = ref_or_id(&repo, &opt.compare).context("unable to find compare")?;
    let compare_tree = repo.find_commit(compare)?.tree()?;
    let merge_base = repo
        .merge_base(base, compare)
        .context("unable to find merge base")?;
    let merge_base_tree = repo.find_commit(merge_base)?.tree()?;
    if opt.verbose {
        info!("merge base: {:?}", merge_base);
    }
    let diff = repo.diff_tree_to_tree(Some(&merge_base_tree), Some(&compare_tree), None)?;
    // diff.print(DiffFormat::Patch, |delta, hunk, line| {
    //     println!("{:?} {:?} {:?}", delta, hunk, line);
    //     true
    // })?;
    // println!("diff: {:?}", diff.stats()?);
    let mut blame_cache: Option<(PathBuf, Result<Blame, git2::Error>)> = None;
    diff.foreach(
        // file_cb
        &mut |delta: DiffDelta, _| {
            info!(
                "from: {:?} to: {:?}",
                delta.old_file().path(),
                delta.new_file().path()
            );
            true
        },
        // binary_cb
        None,
        // hunk_cb
        None,
        // line_cb
        Some(
            &mut |delta: DiffDelta, _hunk: Option<DiffHunk>, line: DiffLine| {
                if let Some(path) = delta.old_file().path() {
                    if blame_cache.is_none() || blame_cache.as_ref().unwrap().0 != path {
                        let newblame = repo.blame_file(path, None);
                        if let Err(ref e) = newblame {
                            warn!("error blaming {:?}: {}", path, e);
                        }
                        blame_cache = Some((path.to_path_buf(), newblame));
                    }
                    if let Ok(blame) = &blame_cache.as_ref().unwrap().1 {
                        print!("{}", String::from_utf8_lossy(line.content()));
                    }
                }
                true
            },
        ),
    )?;
    Ok(())
}
