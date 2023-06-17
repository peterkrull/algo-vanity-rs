use std::{
    thread,
    fs::File,
    io::Write,
    fmt::Display,
    num::NonZeroUsize,
    time::{Instant, Duration},
    sync::{Arc,mpsc,atomic::{AtomicBool, Ordering}},
};

use ctrlc;
use serde_json;
use clap::Parser;
use rand::{Rng,thread_rng};
use serde::{Serialize,Deserialize};
use algo_rust_sdk::account::Account;

/// Number of per-thread account checks between notifying main thread
const COUNT_PER_LOOP: usize = 100;
const COUNT_INTERVAL: usize = COUNT_PER_LOOP*COUNT_PER_LOOP;

/// Default file path to save vanity addresses to
const DEFAULT_PATH: &str = "./vanities.json";

// Maximum number of threads before stopping user
const MAX_THREADS: usize = 128;

// Default number of threads if auto detect fails
const DEFAULT_THREADS: usize = 4;

// Command line arguments.
#[derive(Parser,Debug)]
struct Cli {
    /// Vanity strings to search for (or json file path)
    #[clap(num_args = 1..,required = true)]
    vanities: Vec<String>,

    /// Number of threads (auto detects by default)
    #[clap(short, long)]
    threads: Option<usize>,

    /// Look for match at start of address (default)
    #[clap(short, long,default_value_t = false)]
    start: bool,

    /// Look for match anywhere in address
    #[clap(short, long,default_value_t = false)]
    anywhere: bool,

    /// Look for match at end of address
    #[clap(short, long,default_value_t = false)]
    end: bool,

    /// File path for saving vanity addresses
    #[clap(short, long)]
    path: Option<String>,

    /// Exit after finding each vanity pattern once
    #[clap(short, long, default_value_t = false)]
    once: bool
}

fn main() {

    let mut args = Cli::parse();

    if let Some(t) = args.threads {
        if t > MAX_THREADS {
            println!("{t} threads seems a bit excessive. Perhaps fewer could do?");
            return;
        }
    }

    // Determine number of threads to use, print to user
    let num_threads = args.threads.unwrap_or_else(||{
        thread::available_parallelism().unwrap_or_else(|_e|{
            NonZeroUsize::new(DEFAULT_THREADS).expect("You may perceive this error as a threat")
        }).get()
    });
    println!("Running on {num_threads} threads");

    // Default to searching in start if nothing is
    let path = args.path.unwrap_or(DEFAULT_PATH.to_string());

    // Default to searching in start if nothing is specified
    if (args.start | args.anywhere | args.end ) == false {
        args.start = true;
    }

    // Collect search placement and inform user
    let placement = SearchPlacement { start: args.start, anywhere: args.anywhere, end: args.end };
    println!("Searching at: {placement}");

    // Attempt to load first argument as json file
    if let Ok(file) = File::open(&args.vanities.get(0).expect("Vanity vector must not be empty")) {
        args.vanities = serde_json::from_reader::<_,Vec<String>>(&file)
        .expect("Unable to parse pattern file. Json decoding failed");
    }

    // Ensure all patterns are upper-case
    args.vanities.iter_mut().for_each(|s|{*s = s.to_uppercase()});

    // Ensure all patterns are valid
    let mut invalid_patterns = false;
    let allowed_chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    args.vanities.iter().for_each(|vanity|{
        vanity.chars().into_iter().for_each(|c|{
            if ! allowed_chars.contains(c) {
                invalid_patterns = true;
                println!("Pattern {vanity} contains '{c}' which can not exist in an Algorand Address")
            }
        })
    });
    if invalid_patterns { println!("Exiting due to invalid pattern(s)"); return }
    println!("Looking for patterns:\n{:?}",args.vanities);

    // Atomic boolean to keep worker threads alive
    let keep_alive = Arc::new(AtomicBool::new(true));

    // Setup communication channels between threads
    let (state_tx,state_rx) = mpsc::channel::<WorkerMsg>();
    let (print_tx,print_rx) = mpsc::channel::<PrinterMsg>();
    let (saver_tx,saver_rx) = mpsc::channel::<AddressMatch>();

    // Create and configure worker threads
    let mut worker_handles:Vec<_> = (0..num_threads).map(|id|{
        let keep_alive = keep_alive.clone();

        let state_tx = state_tx.clone();
        let targets = args.vanities.clone();
        let placement = placement.clone();

        thread::spawn(move || {
            thread_worker(id,state_tx,targets,keep_alive,placement);
            println!("Terminated thread [worker {}]",id)
        })
    }).collect();

    let keep_alive_clone = keep_alive.clone();
    worker_handles.push(thread::spawn(move||{
        thread_main_loop(state_rx,saver_tx,print_tx,args.vanities.clone(),args.once,keep_alive_clone,num_threads);
        println!("Terminated thread [main_loop]")
    }));

    let keep_alive_clone = keep_alive.clone();
    worker_handles.push(thread::spawn(move||{
        thread_info_printer(print_rx, keep_alive_clone);
        println!("Terminated thread [info_printer]")
    }));

    worker_handles.push(thread::spawn(move||{
        thread_file_handler(saver_rx,path);
        println!("Terminated thread [file_handler]")
    }));

    // Handle keyboard interrupt by setting keep_alive flag false
    ctrlc::set_handler(move || {
        keep_alive.store(false, Ordering::Relaxed);
        println!("Keyboard interrupt, exiting");
    }).expect("Error setting Ctrl-C handler");


    // Wait for all threads to finish
    for handle in worker_handles {
        let _ = handle.join();
    }

    println!("Exited gracefully")
}

enum PrinterMsg {
    SearchRate(f32),
    TotalCount(usize),
    MatchCount(usize),
}

struct PrinterInfo {
    time:Duration,
    search_rate:f32,
    total_count:usize,
    match_count:usize,
}

enum WorkerMsg {
    AddressMatch(AddressMatch),
    Count((usize,Duration))
}

fn thread_main_loop(
    state_rx : mpsc::Receiver<WorkerMsg>,
    saver_tx : mpsc::Sender<AddressMatch>,
    print_tx : mpsc::Sender<PrinterMsg>,
    mut targets : Vec<String>,
    once : bool,
    keep_alive: Arc<AtomicBool>,
    num_threads:usize
) {
    let mut total_count = 0;
    let mut match_count = 0;
    let mut search_rate = 0.0;
    let mut rates = vec![0.0;num_threads];
    while keep_alive.load(Ordering::Relaxed) {

        match state_rx.recv_timeout(Duration::from_millis(100)) {

            // Address match has been found
            Ok(WorkerMsg::AddressMatch(address_match)) => {

                fn transmit_match (match_count: &mut usize,saver_tx: &mpsc::Sender<AddressMatch>,print_tx: &mpsc::Sender<PrinterMsg>,address_match: AddressMatch) {
                    *match_count += 1;
                    saver_tx.send(address_match).expect("Unable to transmit address match from main thread");
                    print_tx.send(PrinterMsg::MatchCount(*match_count)).expect("Unable to transmit match count from main thread");
                }

                if once {
                    if let Some(index) = targets.iter().position(|r| r == &address_match.target)  {
                        transmit_match(&mut match_count,&saver_tx, &print_tx, address_match);
                        let _removed = targets.remove(index);
                        if targets.len() == 0 {
                            println!("Found all vanity addresses!");
                            keep_alive.store(false,Ordering::Relaxed)
                        }
                    }
                } else {
                    transmit_match(&mut match_count,&saver_tx,&print_tx, address_match);
                }

            },

            // Worker thread counting update
            Ok(WorkerMsg::Count((id,duration))) => {
                total_count += COUNT_INTERVAL;
                rates[id] = COUNT_INTERVAL as f32 / duration.as_secs_f32();
                search_rate = search_rate*0.95 + rates.iter().sum::<f32>()*0.05;
                print_tx.send(PrinterMsg::SearchRate(search_rate)).expect("Unable to transmit rate from main thread");
                print_tx.send(PrinterMsg::TotalCount(total_count)).expect("Unable to transmit total count from main thread");
            },
            _ => {}
        }
    }
}



fn thread_worker(
    id:usize,
    sender : mpsc::Sender<WorkerMsg>,
    targets : Vec<String>,
    keep_alive: Arc<AtomicBool>,
    placement : SearchPlacement
) {
    let mut prev_time = Instant::now();
    let mut rng = thread_rng();
    while keep_alive.load(Ordering::Relaxed) {

        // This hack allows for only generating orders of magnitudes fewer random numbers.
        // After generating the first seed, we generate two random numbers which represent
        // two indeces of the seed. These indeces are counted up in the for loops to change
        // the seed ever so slightly. For loops and counting is much faster than generating
        // 32 new random numbers every time. The same perturbed seed is used COUNT_PER_LOOP^2
        // times before a new seed is generated. By default this is 10_000 times.

        let mut seed: [u8; 32] = rng.gen();
        let index0: u8 = rng.gen_range(0..32);
        let index1: u8 = rng.gen_range(0..32);
        for _ in 0..COUNT_PER_LOOP {
            seed[index0 as usize] = seed[index0 as usize].wrapping_add(1);
            for _ in 0..COUNT_PER_LOOP {
                seed[index1 as usize] = seed[index1 as usize].wrapping_add(1);
                let acc = Account::from_seed(seed);
                find_vanity(&sender, &targets, &acc, &placement);
            }
        }

        let current_time = Instant::now();
        let duration = Instant::now().duration_since(prev_time);
        prev_time = current_time;
        let _ = sender.send(WorkerMsg::Count((id,duration)));
    }
}

fn thread_info_printer(receiver : mpsc::Receiver<PrinterMsg>, keep_alive: Arc<AtomicBool>) {

    let start_time = Instant::now();

    let mut info_state = PrinterInfo {
        time: Duration::default(),
        search_rate: 0.0,
        total_count: 0,
        match_count: 0,
    };

    while keep_alive.load(Ordering::Relaxed) {

        // Receive all updated values before printing
        while let Ok(message) = receiver.try_recv() {
            match message {
                PrinterMsg::SearchRate(v) => info_state.search_rate = v,
                PrinterMsg::TotalCount(v) => info_state.total_count = v,
                PrinterMsg::MatchCount(v) => info_state.match_count = v,
            }
        }

        info_state.time = Instant::now().duration_since(start_time);

        let sec = info_state.time.as_secs() % 60;
        let min = (info_state.time.as_secs() / 60) % 60;
        let hrs = (info_state.time.as_secs() / 60) / 60;

        println!("\n\nStats for current session");
        println!("Timer: {}h:{:02}m:{:02}s",hrs,min,sec);
        println!("Speed: {:.0} a/s",info_state.search_rate);
        println!("Total: {:.2} million",info_state.total_count as f32 / 1e6);
        println!("Match: {}",info_state.match_count);

        thread::sleep(Duration::from_secs(1));
    }
}

#[derive(Clone,Debug)]
struct SearchPlacement {
    start:bool,
    anywhere:bool,
    end:bool,
}

impl Display for SearchPlacement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f,"{}",match (self.start,self.anywhere,self.end) {
            (_, true, _) => "Anywhere",
            (true, false, true) => "Start and end",
            (true, false, false) => "Start",
            (false, false, true) => "End",
            (false, false, false) => "Nowhere"
        })
    }
}

/// Struct for when an address has matched a vanity string
#[derive(Serialize,Deserialize)]
struct AddressMatch {
    target : String,
    public : String,
    mnemonic : String,
    placement : Placement
}

#[derive(Serialize,Deserialize)]
enum Placement {
    Anywhere(usize),
    Start,
    End,
}

/// Threads to handle saving matches to json file
fn thread_file_handler(receiver : mpsc::Receiver<AddressMatch>, path:String) {

    // Load existing vanity json or create a new one
    let mut matches = if let Ok(file) = File::open(&path) {
        serde_json::from_reader(&file).expect("Unable to parse existing file")
    } else {
        let file = File::create(&path).expect("Unable to create new file");
        serde_json::to_writer(&file, &[0;0]).expect("Unable to write to file");
        Vec::new()
    };

    // Receive new address match, add it to vector and save to disk
    while let Ok(message) = receiver.recv() {

        matches.push(message);
        matches.append(& mut receiver.try_iter().collect());

        if let Ok(json_message) = serde_json::to_string_pretty(&matches) {
            let mut file = File::create(&path).expect("Unable to open file as writeable");
            write!(file,"{}", json_message.as_str()).expect("Unable to write json string to file");
        }
    }
}

fn find_vanity(sender : &mpsc::Sender<WorkerMsg>,targets : &Vec<String>,acc : &Account, placement : &SearchPlacement ) {
    let acc_string = acc.address().encode_string();
    for target in targets {

        let mut matched_start_end = false;

        // Look for match at start of address
        if placement.start && acc_string.starts_with(target.as_str()) {
            let _ = sender.send(
                WorkerMsg::AddressMatch(AddressMatch {
                    target: target.clone(),
                    public: acc_string.clone(),
                    mnemonic: acc.mnemonic(),
                    placement: Placement::Start
                })
            );
            matched_start_end = true;
        }

        // Look for match at end of address
        if placement.end && acc_string.ends_with(target.as_str()) {
            let _ = sender.send(
                WorkerMsg::AddressMatch(AddressMatch {
                    target: target.clone(),
                    public: acc_string.clone(),
                    mnemonic: acc.mnemonic(),
                    placement: Placement::End
                })
            );
            matched_start_end = true;
        }

        // Look for match anywhere in address
        if matched_start_end == false && placement.anywhere {
            if let Some(index) = acc_string.find(target.as_str()) {
                let _ = sender.send(
                    WorkerMsg::AddressMatch(AddressMatch {
                        target: target.clone(),
                        public: acc_string.clone(),
                        mnemonic: acc.mnemonic(),
                        placement: Placement::Anywhere(index)
                    })
                );
            }
        }
    };
}