#![cfg_attr(feature = "trace", feature(const_type_name))]

use hvmc::{run::Mode, *};

use std::{
  collections::HashSet,
  env, fs, io,
  path::Path,
  process::{self, Stdio},
  sync::{Arc, Mutex},
  time::{Duration, Instant},
};

fn main() {
  if cfg!(feature = "trace") {
    trace::set_hook();
  }
  let args: Vec<String> = env::args().skip(1).collect();
  if cfg!(feature = "_full_cli") {
    full_main(&args)
  } else {
    run(&args, Arc::new(Mutex::new(hvmc::gen::host())))
  }
  if cfg!(feature = "trace") {
    hvmc::trace::_read_traces(usize::MAX);
  }
}

fn full_main(args: &[String]) {
  let action = args.get(0).map(|x| &**x).unwrap_or("help");
  let file_name = args.get(1);
  let opts = &args.get(2 ..).unwrap_or(&[]);
  match action {
    "run" => {
      let Some(file_name) = file_name else {
        println!("Usage: hvmc run <file.hvmc> [-s] [-1]");
        process::exit(1);
      };
      let host = load(file_name);
      run(opts, host);
    }
    "compile" => {
      let Some(file_name) = file_name else {
        println!("Usage: hvmc compile <file.hvmc>");
        process::exit(1);
      };
      let host = load(file_name);
      compile_executable(file_name, &host.lock().unwrap()).unwrap();
    }
    _ => {
      println!("Usage: hvmc <cmd> <file.hvmc> [-s]");
      println!("Commands:");
      println!("  run           - Run the given file");
      println!("  compile       - Compile the given file to an executable");
      println!("Options:");
      println!("  [-s] Show stats, including rewrite count");
      println!("  [-1] Single-core mode (no parallelism)");
    }
  }
}

fn run(opts: &[String], host: Arc<Mutex<host::Host>>) {
  let opts = opts.iter().map(|x| &**x).collect::<HashSet<_>>();
  let data = run::Net::<run::Strict>::init_heap(1 << 32);
  let lazy = opts.contains("-L");
  let net = run::DynNet::new(&data, lazy);
  dispatch_dyn_net! { mut net => {
    net.boot(&host.lock().unwrap().defs["main"]);
    let start_time = Instant::now();
    if lazy || opts.contains("-1") {
      net.normal();
    } else {
      net.parallel_normal();
    }
    let elapsed = start_time.elapsed();
    println!("{}", &host.lock().unwrap().readback(&net));
    if opts.contains("-s") {
      print_stats(&net, elapsed);
    }
  } }
}

fn print_stats<M: Mode>(net: &run::Net<M>, elapsed: Duration) {
  eprintln!("RWTS   : {:>15}", pretty_num(net.rwts.total()));
  eprintln!("- ANNI : {:>15}", pretty_num(net.rwts.anni));
  eprintln!("- COMM : {:>15}", pretty_num(net.rwts.comm));
  eprintln!("- ERAS : {:>15}", pretty_num(net.rwts.eras));
  eprintln!("- DREF : {:>15}", pretty_num(net.rwts.dref));
  eprintln!("- OPER : {:>15}", pretty_num(net.rwts.oper));
  eprintln!("TIME   : {:.3?}", elapsed);
  eprintln!("RPS    : {:.3} M", (net.rwts.total() as f64) / (elapsed.as_millis() as f64) / 1000.0);
}

fn pretty_num(n: u64) -> String {
  n.to_string()
    .as_bytes()
    .rchunks(3)
    .rev()
    .map(|x| std::str::from_utf8(x).unwrap())
    .flat_map(|x| ["_", x])
    .skip(1)
    .collect()
}

fn load(file: &str) -> Arc<Mutex<host::Host>> {
  let Ok(file) = fs::read_to_string(file) else {
    eprintln!("Input file not found");
    process::exit(1);
  };
  let host = Arc::new(Mutex::new(host::Host::default()));
  host.lock().unwrap().insert_def(
    "HVM.log",
    host::DefRef::Owned(Box::new(stdlib::LogDef::new({
      let host = Arc::downgrade(&host);
      move |wire| {
        println!("{}", host.upgrade().unwrap().lock().unwrap().readback_tree(&wire));
      }
    }))),
  );
  host.lock().unwrap().insert_book(&file.parse().expect("parse error"));
  host
}

fn compile_executable(file_name: &str, host: &host::Host) -> Result<(), io::Error> {
  let gen = compile::compile_host(host);
  let outdir = ".hvm";
  if Path::new(&outdir).exists() {
    fs::remove_dir_all(outdir)?;
  }
  let cargo_toml = include_str!("../Cargo.toml");
  let cargo_toml = cargo_toml.split("##--COMPILER-CUTOFF--##").next().unwrap();
  fs::create_dir_all(format!("{}/src", outdir))?;
  fs::write(".hvm/Cargo.toml", cargo_toml)?;
  fs::write(".hvm/src/ast.rs", include_str!("../src/ast.rs"))?;
  fs::write(".hvm/src/fuzz.rs", include_str!("../src/fuzz.rs"))?;
  fs::write(".hvm/src/host.rs", include_str!("../src/host.rs"))?;
  fs::write(".hvm/src/compile.rs", include_str!("../src/compile.rs"))?;
  fs::write(".hvm/src/lib.rs", include_str!("../src/lib.rs"))?;
  fs::write(".hvm/src/main.rs", include_str!("../src/main.rs"))?;
  fs::write(".hvm/src/ops.rs", include_str!("../src/ops.rs"))?;
  fs::write(".hvm/src/run.rs", include_str!("../src/run.rs"))?;
  fs::write(".hvm/src/stdlib.rs", include_str!("../src/stdlib.rs"))?;
  fs::write(".hvm/src/trace.rs", include_str!("../src/trace.rs"))?;
  fs::write(".hvm/src/util.rs", include_str!("../src/util.rs"))?;
  fs::write(".hvm/src/gen.rs", gen)?;

  let output = process::Command::new("cargo")
    .current_dir("./.hvm")
    .arg("build")
    .arg("--release")
    .stderr(Stdio::inherit())
    .output()?;
  if !output.status.success() {
    process::exit(1);
  }

  let target = format!("./{}", file_name.strip_suffix(".hvmc").unwrap_or(file_name));
  fs::copy("./.hvm/target/release/hvmc", target)?;

  Ok(())
}
