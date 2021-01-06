use anyhow::Result;
use structopt::StructOpt;
use git2::Repository;

#[derive(Debug, StructOpt)]
#[structopt(about = "List authors of lines changed by PR")]
struct Opt {
    /// where to merge from
    #[structopt()]
    compare: String,

    /// where to merge to
    #[structopt()]
    base: String,
}

fn main() -> Result<()> {
    let opt = Opt::from_args();
    let repo = Repository::open(".")?;
    println!("{:?}", repo.head()?.name());
    Ok(())
}
