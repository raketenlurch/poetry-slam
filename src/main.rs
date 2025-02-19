#[macro_use]
extern crate rocket;

use serde::Serialize;

use std::{
    collections::BTreeSet,
    error::Error,
    path::{Path, PathBuf},
    thread::spawn,
};

use flume::{Receiver, Sender};
use printer::PoetryPrinter;
use rocket::{form::Form, fs::NamedFile, Build, Rocket, State};
use rocket_dyn_templates::Template;

mod poem_generator;
mod printer;
mod training_sets;

struct PrinterArgs {
    name: String,
    poem: String,
}

struct Print {
    join_handle: std::thread::JoinHandle<()>,
    poem_tx: Sender<PrinterArgs>,
}

#[derive(FromForm)]
struct PoemGenerationForm<'r> {
    training_data: &'r str,
    name: &'r str,
    print_and_hide: bool,
}
#[derive(Serialize)]
struct TemplateContext<'a> {
    name: &'a str,
    training_data: &'a str,
    training_sets: &'a BTreeSet<&'a str>,
    poem: Option<String>,
}

impl<'a> TemplateContext<'a> {
    pub fn new(
        poem_generation: Option<PoemGenerationForm<'a>>,
        poem: Option<String>,
    ) -> TemplateContext<'a> {
        TemplateContext {
            name: poem_generation
                .as_ref()
                .map_or_else(|| "", |poem_generation| poem_generation.name),
            training_data: poem_generation
                .as_ref()
                .map_or_else(|| "", |poem_generation| poem_generation.training_data),
            training_sets: &training_sets::TRAINING_SETS,
            poem,
        }
    }
}

#[get("/")]
fn index() -> Template {
    Template::render("index", TemplateContext::new(None, None))
}

#[post("/", data = "<poem_generation>")]
async fn generate(
    poem_generation: Form<PoemGenerationForm<'_>>,
    poem_tx: &State<Option<Sender<PrinterArgs>>>,
) -> Result<Template, String> {
    // hmm. this generates a 200 in case of an error :S
    let poem = poem_generator::generate(poem_generation.training_data)
        .await
        .map_err(|e| e.to_string())?;

    let poem = if poem_generation.print_and_hide {
        if let Some(poem_tx) = poem_tx.inner() {
            poem_tx
                .send(PrinterArgs {
                    name: poem_generation.name.to_string(),
                    poem,
                })
                .map_err(|e| e.to_string())?;
        }
        None
    } else {
        Some(poem)
    };

    Ok(Template::render(
        "index",
        TemplateContext::new(Some(poem_generation.into_inner()), poem),
    ))
}

#[get("/assets/<asset..>")]
async fn get_asset(asset: PathBuf) -> Option<NamedFile> {
    NamedFile::open(Path::new("assets/").join(asset)).await.ok()
}

fn rocket(poem_tx: Option<Sender<PrinterArgs>>) -> Rocket<Build> {
    rocket::build()
        .mount("/", routes![index, generate, get_asset])
        .manage(poem_tx)
        .attach(Template::fairing())
}

#[rocket::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let print = match PoetryPrinter::new() {
        Ok(mut printer) => {
            let (poem_tx, poem_rx): (Sender<PrinterArgs>, Receiver<PrinterArgs>) =
                flume::unbounded();
            let join_handle = spawn(move || loop {
                let args = match poem_rx.recv() {
                    Ok(args) => args,
                    Err(e) => {
                        println!("Printer thread exited : {}", e);
                        return;
                    }
                };
                if let Err(e) = printer.print_poem(&args.name, &args.poem) {
                    println!("Printing failed : {}", e);
                }
            });
            Some(Print {
                join_handle,
                poem_tx,
            })
        }
        Err(e) => {
            eprintln!("Printer init failed: {}. Skipping print.", e);
            None
        }
    };
    if let Some(print) = print {
        let _ = rocket(Some(print.poem_tx)).launch().await;
        print.join_handle.join().unwrap();
    } else {
        let _ = rocket(None).launch().await;
    }

    Ok(())
}
