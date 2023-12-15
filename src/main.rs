use std::{
    thread,
    fs::File,
    io::{Write, self},
    fmt::Display,
    time::{Instant, Duration},
    sync::{Arc,mpsc,atomic::{AtomicBool, Ordering}, Mutex},
};

use clap::Parser;
use rand::{Rng,thread_rng};
use serde::{Serialize,Deserialize};
use algo_rust_sdk::account::Account;

mod tui;

/// Number of per-thread account checks between notifying main thread
const COUNT_PER_LOOP: usize = 100;

/// Default file path to save vanity addresses to
const DEFAULT_PATH: &str = "vanities.json";

// Maximum number of threads before stopping user
const MAX_THREADS: usize = 128;

// Default number of threads if auto detect fails
const DEFAULT_THREADS: usize = 4;

/// Message types worker threads send back to the main thread loop
enum WorkerMsg {
    AddressMatch(AddressMatch),
    Count((usize,Duration))
}

/// Struct for when an address has matched a vanity string
#[derive(Serialize,Deserialize,Clone)]
struct AddressMatch {
    target : String,
    public : String,
    mnemonic : String,
    placement : Placement
}

/// Placement of matched string pattern
#[derive(Serialize,Deserialize,Clone)]
enum Placement {
    Start,
    Anywhere(usize),
    End,
}

struct GlobalState {
    vanities: Vec<String>,
    threads: usize,
    placement: SearchPlacement,
    matches: Vec<AddressMatch>,
    search_rate: f32,
    total_count: usize,
    match_count: usize,
    start_time: Instant,
    run_time: Duration,
    save_path: String,
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

    // Check for realistic number of threads (fewer than MAX_THREADS)
    let num_threads = match args.threads {
        Some(t @ 1..=MAX_THREADS) => t, // Valid number of threads requested
        Some(0) => { println!("Error: User requested 0 threads, please select 1 or more"); return },
        Some(t) => { println!("Error: User requested {t} threads, please select {MAX_THREADS} or fewer"); return },
        None => thread::available_parallelism().map_or(DEFAULT_THREADS, |t|t.get())
    };
    
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

    // Atomic boolean to keep worker threads alive
    let keep_alive = Arc::new(AtomicBool::new(true));

    // Initialize system state
    let state = Arc::new(Mutex::new(GlobalState{
        vanities: args.vanities.clone(),
        threads: num_threads,
        placement: placement.clone(),
        matches: Vec::new(),
        search_rate: 0.0f32,
        total_count: 0usize,
        match_count: 0usize,
        start_time: Instant::now(),
        run_time: Duration::ZERO,
        save_path: save_path.clone(),
    }));

    // Configure and create threads
    let thread_handles = {

        // Setup communication channels between threads
        let (tx_worker_msg,rx_worker_msg) = mpsc::channel::<WorkerMsg>();
        let (tx_address_match,rx_address_match) = mpsc::channel::<AddressMatch>();

        // Setup worker threads (num_threads of them)
        let mut thread_handles:Vec<_> = (0..num_threads).map(|thread_id|{
            
            let tx_worker_msg_clone = tx_worker_msg.clone();
            let state_clone = state.clone();
            let keep_alive_clone = keep_alive.clone();
            let placement_clone = placement.clone();

            thread::spawn(move || {
                thread_worker(thread_id,tx_worker_msg_clone, state_clone, keep_alive_clone, placement_clone);
                println!("Terminated thread [worker {}]",thread_id)
            })
        }).collect();

        // Setup main loop thread
        let keep_alive_clone = keep_alive.clone();
        let state_clone = state.clone();
        thread_handles.push(thread::spawn(move||{
            thread_main_loop(rx_worker_msg, tx_address_match, state_clone, args.once, keep_alive_clone);
            println!("Terminated thread [main_loop]")
        }));

        // Setup file handler thread
        let keep_alive_clone = keep_alive.clone();
        thread_handles.push(thread::spawn(move||{
            if let Err(e) = thread_file_handler(rx_address_match, save_path) {
                keep_alive_clone.store(false,Ordering::Relaxed);
                println!("Error: Unable to save vanity addresses to file: {}",e);
            }
            println!("Terminated thread [file_handler]")
        }));

        // Setup user interface thread
        let keep_alive_clone = keep_alive.clone();
        let state_clone = state.clone();
        thread_handles.push(thread::spawn(move||{
            // Wait for other threads to start
            thread::sleep(Duration::from_millis(10));
            _ = tui::main(&state_clone, keep_alive_clone);
            println!("Terminated thread [info_printer]")
        }));

        // return thread handles
        thread_handles
    };

    // Wait for all threads to finish
    for handle in thread_handles {
        _ = handle.join();
    }

    println!("All threads finished, exiting")
}

fn thread_main_loop(
    rx_worker_msg: mpsc::Receiver<WorkerMsg>,
    tx_address_match: mpsc::Sender<AddressMatch>,
    state: Arc<Mutex<GlobalState>>,
    find_only_once: bool,
    keep_alive: Arc<AtomicBool>,
) {

    let mut rates = vec![0.0;state.lock().expect("Unable to lock mutex").threads];
    while let Ok(msg) = rx_worker_msg.recv() {

        let mut state_mut = state.lock().expect("Unable to lock mutex");

        state_mut.run_time = Instant::now().duration_since(state_mut.start_time);

        match msg {

            // Address match has been found
            WorkerMsg::AddressMatch(address_match) => {

                state_mut.matches.push(address_match.clone());

                if find_only_once {
                    if let Some(index) = state_mut.vanities.iter().position(|r| r == &address_match.target)  {
                        state_mut.match_count += 1;
                        _ = tx_address_match.send(address_match);
                        let _removed = state_mut.vanities.remove(index);
                        if state_mut.vanities.is_empty() {
                            println!("Found all vanity addresses!");
                            keep_alive.store(false,Ordering::Relaxed)
                        }
                    }
                } else {
                    state_mut.match_count += 1;
                    _ = tx_address_match.send(address_match);
                }

            },

            // Worker thread counting update
            WorkerMsg::Count((id,duration)) => {
                state_mut.total_count += COUNT_PER_LOOP * COUNT_PER_LOOP ;
                rates[id] = (COUNT_PER_LOOP * COUNT_PER_LOOP) as f32 / duration.as_secs_f32();
                state_mut.search_rate = state_mut.search_rate*0.95 + rates.iter().sum::<f32>()*0.05; // LP-filtered rate
            },
        }
    }
}

fn thread_worker(
    thread_id: usize,
    tx_worker_msg: mpsc::Sender<WorkerMsg>,
    state: Arc<Mutex<GlobalState>>,
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
        let vanity_targets = if let Ok(s) = state.lock() { s.vanities.clone() } else { return };
        for _ in 0..COUNT_PER_LOOP {
            seed[index0 as usize] = seed[index0 as usize].wrapping_add(3);
            for _ in 0..COUNT_PER_LOOP {
                seed[index1 as usize] = seed[index1 as usize].wrapping_add(3);
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

/// Threads to handle saving matches to json file
fn thread_file_handler(
    rx_address_match: mpsc::Receiver<AddressMatch>,
    path: String
) -> io::Result<()> {

    // Load existing vanity json or create a new one
    let mut matches = if let Ok(file) = File::open(&path) {
        serde_json::from_reader(&file)?
    } else {
        let file = File::create(&path)?;
        serde_json::to_writer(&file, &[0;0])?;
        Vec::new()
    };

    // Receive new address match, add it to vector and save to disk
    while let Ok(message) = rx_address_match.recv() {

        matches.push(message);
        matches.append(& mut rx_address_match.try_iter().collect());

        if let Ok(json_message) = serde_json::to_string_pretty(&matches) {
            let mut file = File::create(&path)?;
            write!(file,"{}", json_message.as_str())?;
        }
    }

    Ok(())
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
