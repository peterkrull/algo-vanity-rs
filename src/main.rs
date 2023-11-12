#![feature(let_chains)]

use std::{
    thread,
    fs::File,
    io::Write,
    fmt::Display,
    time::{Instant, Duration},
    sync::{Arc,mpsc,atomic::{AtomicBool, Ordering}},
};

use clap::Parser;
use rand::{Rng,thread_rng};
use serde::{Serialize,Deserialize};
use algo_rust_sdk::account::Account;

/// Number of per-thread account checks between notifying main thread
const COUNT_PER_LOOP: usize = 100;

/// Default file path to save vanity addresses to
const DEFAULT_PATH: &str = "./vanities.json";

// Maximum number of threads before stopping user
const MAX_THREADS: usize = 128;

// Default number of threads if auto detect fails
const DEFAULT_THREADS: usize = 4;

/// Message types that can be sent to info printer thread
enum PrinterMsg {
    SearchRate(f32),
    TotalCount(usize),
    MatchCount(usize),
}

/// Message types worker theads send back to the main thread loop
enum WorkerMsg {
    AddressMatch(AddressMatch),
    Count((usize,Duration))
}

/// Struct for when an address has matched a vanity string
#[derive(Serialize,Deserialize)]
struct AddressMatch {
    target : String,
    public : String,
    mnemonic : String,
    placement : Placement
}

/// Placement of matched string pattern
#[derive(Serialize,Deserialize)]
enum Placement {
    Start,
    Anywhere(usize),
    End,
}

/// Places to search in addresses
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
    #[clap(short, long, default_value_t = false)]
    start: bool,

    /// Look for match anywhere in address
    #[clap(short, long, default_value_t = false)]
    anywhere: bool,

    /// Look for match at end of address
    #[clap(short, long, default_value_t = false)]
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

    // Check for realistic number of threads
    if let Some(t @ MAX_THREADS..) = args.threads {
        println!("Error: User requested {t} threads, please select fewer than {MAX_THREADS}"); return
    }

    // Number of threads to use: cli arg > num cpus > default value
    let num_threads = args.threads.unwrap_or_else(||{
        thread::available_parallelism().map_or(DEFAULT_THREADS, |t|t.get())
    });
    
    // String representing path for saving vanities
    let save_path = args.path.unwrap_or(DEFAULT_PATH.to_string());

    // Default to searching in start if nothing is specified
    if !(args.start | args.anywhere | args.end ) {
        args.start = true;
    }

    // Collect search placement and inform user
    let placement = SearchPlacement { start: args.start, anywhere: args.anywhere, end: args.end };

    // Attempt to load first argument as json file
    let file_name = args.vanities.first().expect("Clap struct entry 'vanities' is must contain one or more elements.");
    if let Ok(file) = File::open(file_name) {
        args.vanities = if let Ok(vanities_from_file) = serde_json::from_reader::<_,Vec<String>>(&file) { vanities_from_file }
        else { println!("Error: Unable to parse file as valid JSON of correct format, e.g. [\"algo\",\"rand\"]"); return }
    }

    // Ensure all patterns are upper-case
    args.vanities.iter_mut().for_each(|s|{*s = s.to_uppercase()});

    // Ensure all patterns are valid
    let mut invalid_patterns = false;
    let allowed_chars = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    args.vanities.iter().for_each(|vanity|{vanity.chars().for_each(|c|{
        if ! allowed_chars.contains(c) {
            invalid_patterns = true;
            println!("Pattern {vanity} contains '{c}' which can not exist in an Algorand Address")
        }
    })});
    if invalid_patterns { println!("Error: Exiting due to invalid pattern(s)"); return }

    // Print summary
    println!("Running on {num_threads} threads");
    println!("Searching at: {placement}");
    println!("Looking for patterns:\n{:?}",args.vanities);

    // Atomic boolean to keep worker threads alive
    let keep_alive = Arc::new(AtomicBool::new(true));

    // Setup communication channels between threads
    let (tx_worker_msg,rx_worker_msg) = mpsc::channel::<WorkerMsg>();
    let (tx_printer_msg,rx_printer_msg) = mpsc::channel::<PrinterMsg>();
    let (tx_address_match,rx_address_match) = mpsc::channel::<AddressMatch>();

    // Configure and create threads
    let mut thread_handles:Vec<_> = (0..num_threads).map(|thread_id|{
        let keep_alive = keep_alive.clone();

        let tx_worker_msg = tx_worker_msg.clone();
        let vanity_targets = args.vanities.clone();
        let placement = placement.clone();

        thread::spawn(move || {
            thread_worker(thread_id,tx_worker_msg,vanity_targets, keep_alive, placement);
            println!("Terminated thread [worker {}]",thread_id)
        })
    }).collect();

    let keep_alive_clone = keep_alive.clone();
    thread_handles.push(thread::spawn(move||{
        thread_main_loop(rx_worker_msg, tx_address_match, tx_printer_msg, args.vanities.clone(), args.once, keep_alive_clone, num_threads);
        println!("Terminated thread [main_loop]")
    }));

    let keep_alive_clone = keep_alive.clone();
    thread_handles.push(thread::spawn(move||{
        thread_info_printer(rx_printer_msg, keep_alive_clone);
        println!("Terminated thread [info_printer]")
    }));

    thread_handles.push(thread::spawn(move||{
        thread_file_handler(rx_address_match, save_path);
        println!("Terminated thread [file_handler]")
    }));

    // Handle keyboard interrupt by setting keep_alive flag false
    ctrlc::set_handler(move || {
        keep_alive.store(false, Ordering::Relaxed);
        println!("Keyboard interrupt, exiting");
    }).expect("Error setting Ctrl-C handler");

    // Wait for all threads to finish
    for handle in thread_handles {
        _ = handle.join();
    }

    println!("Exited gracefully")
}

fn thread_main_loop(
    rx_worker_msg: mpsc::Receiver<WorkerMsg>,
    tx_address_match: mpsc::Sender<AddressMatch>,
    tx_printer_msg: mpsc::Sender<PrinterMsg>,
    mut vanity_targets: Vec<String>,
    find_only_once: bool,
    keep_alive: Arc<AtomicBool>,
    num_threads: usize
) {
    let mut total_count = 0;
    let mut match_count = 0;
    let mut search_rate = 0.0;
    let mut rates = vec![0.0;num_threads];
    while keep_alive.load(Ordering::Relaxed) {

        if let Ok(msg) = rx_worker_msg.recv() {

            match msg {

                // Address match has been found
                WorkerMsg::AddressMatch(address_match) => {

                    fn transmit_match (match_count: &mut usize,tx_address_match: &mpsc::Sender<AddressMatch>, tx_printer_msg: &mpsc::Sender<PrinterMsg>, address_match: AddressMatch) {
                        *match_count += 1;
                        tx_address_match.send(address_match).expect("Unable to transmit address match from main thread");
                        tx_printer_msg.send(PrinterMsg::MatchCount(*match_count)).expect("Unable to transmit match count from main thread");
                    }

                    if find_only_once {
                        if let Some(index) = vanity_targets.iter().position(|r| r == &address_match.target)  {
                            transmit_match(&mut match_count,&tx_address_match, &tx_printer_msg, address_match);
                            let _removed = vanity_targets.remove(index);
                            if vanity_targets.is_empty() {
                                println!("Found all vanity addresses!");
                                keep_alive.store(false,Ordering::Relaxed)
                            }
                        }
                    } else {
                        transmit_match(&mut match_count,&tx_address_match,&tx_printer_msg, address_match);
                    }

                },

                // Worker thread counting update
                WorkerMsg::Count((id,duration)) => {
                    total_count += COUNT_PER_LOOP * COUNT_PER_LOOP ;
                    rates[id] = (COUNT_PER_LOOP * COUNT_PER_LOOP) as f32 / duration.as_secs_f32();
                    search_rate = search_rate*0.95 + rates.iter().sum::<f32>()*0.05; // LP-filtered rate
                    tx_printer_msg.send(PrinterMsg::SearchRate(search_rate)).expect("Unable to transmit rate from main thread");
                    tx_printer_msg.send(PrinterMsg::TotalCount(total_count)).expect("Unable to transmit total count from main thread");
                },
            }
        } else {
            break;
        }
    }
}

fn thread_worker(
    thread_id: usize,
    tx_worker_msg: mpsc::Sender<WorkerMsg>,
    vanity_targets: Vec<String>,
    keep_alive: Arc<AtomicBool>,
    placement: SearchPlacement
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
                find_vanity(&tx_worker_msg, &vanity_targets, &acc, &placement);
            }
        }

        let current_time = Instant::now();
        let duration = Instant::now().duration_since(prev_time);
        prev_time = current_time;
        _ = tx_worker_msg.send(WorkerMsg::Count((thread_id,duration)));
    }
}

fn thread_info_printer(
    rx_printer_msg: mpsc::Receiver<PrinterMsg>,
    keep_alive: Arc<AtomicBool>
) {

    let start_time = Instant::now();

    let mut time: Duration;
    let mut search_rate = 0.0;
    let mut total_count = 0;
    let mut match_count = 0;

    while keep_alive.load(Ordering::Relaxed) {

        // Receive all updated values before printing
        while let Ok(message) = rx_printer_msg.try_recv() {
            match message {
                PrinterMsg::SearchRate(v) => search_rate = v,
                PrinterMsg::TotalCount(v) => total_count = v,
                PrinterMsg::MatchCount(v) => match_count = v,
            }
        }

        time = Instant::now().duration_since(start_time);

        let sec = time.as_secs() % 60;
        let min = (time.as_secs() / 60) % 60;
        let hrs = (time.as_secs() / 60) / 60;

        println!("\n\nStats for current session");
        println!("Timer: {}h:{:02}m:{:02}s",hrs,min,sec);
        println!("Speed: {:.0} a/s",search_rate);
        println!("Total: {:.2} million",total_count as f32 / 1e6);
        println!("Match: {}",match_count);

        thread::sleep(Duration::from_secs(1));
    }
}

/// Threads to handle saving matches to json file
fn thread_file_handler(
    rx_address_match: mpsc::Receiver<AddressMatch>,
    path: String
) {

    // Load existing vanity json or create a new one
    let mut matches = if let Ok(file) = File::open(&path) {
        serde_json::from_reader(&file).expect("Unable to parse existing file")
    } else {
        let file = File::create(&path).expect("Unable to create new file");
        serde_json::to_writer(&file, &[0;0]).expect("Unable to write to file");
        Vec::new()
    };

    // Receive new address match, add it to vector and save to disk
    while let Ok(message) = rx_address_match.recv() {

        matches.push(message);
        matches.append(& mut rx_address_match.try_iter().collect());

        if let Ok(json_message) = serde_json::to_string_pretty(&matches) {
            let mut file = File::create(&path).expect("Unable to open file as writeable");
            write!(file,"{}", json_message.as_str()).expect("Unable to write json string to file");
        }
    }
}

fn find_vanity(
    tx_worker_msg: &mpsc::Sender<WorkerMsg>,
    vanity_targets: &Vec<String>,
    acc: &Account,
    placement: &SearchPlacement
) {
    let acc_string = acc.address().encode_string();
    for target in vanity_targets {

        let mut matched_start_end = false;

        // Look for match at start of address
        if placement.start && acc_string.starts_with(target.as_str()) {
            _ = tx_worker_msg.send(
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
            _ = tx_worker_msg.send(
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
        if !matched_start_end && placement.anywhere {
            if let Some(index) = acc_string.find(target.as_str()) {
                _ = tx_worker_msg.send(
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