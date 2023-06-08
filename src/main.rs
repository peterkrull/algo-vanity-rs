use std::{
    thread,
    fs::File,
    io::Write,
    time::{Instant, Duration},
    sync::{Arc,mpsc,atomic::{AtomicBool, Ordering}},
};

use ctrlc;
use clap::Parser;
use serde_json;
use serde::{Serialize,Deserialize};
use algo_rust_sdk::account::Account;

/// Number of per-thread account checks between notifying main thread
const COUNT_INTERVAL : usize = 10_000;

/// File path to save vanity addresses to
const DEFAULT_PATH : &str = "./vanities.json";

/// Command line arguments
#[derive(Parser,Debug)]
struct Cli {
    /// Number of threads
    #[clap(short, long, default_value_t = 0)]
    threads: usize,

    /// Where to look for vanity: { start , anywhere, end }
    #[clap(short, long)]
    location: Vec<String>,

    /// Exit after finding each vanity once
    #[clap(short, long, default_value_t = false)]
    once: bool,

    /// List of vanity strings to search for
    #[clap(num_args = 1..,required = true)]
    vanities: Vec<String>,
}

fn main() {

    let mut args = Cli::parse();

    if args.threads > 128 {
        println!("{} threads seems a bit excessive. Perhaps fewer could do?",args.threads);
        return;
    }

    // Determine number of threads to use, print to user
    if args.threads == 0 {
        if let Ok(num_threads) = thread::available_parallelism() {
            println!("Detected {num_threads} threads automatically");
            args.threads = num_threads.into();
        } else {
            println!("Unable to detect number of cpu threads, defaulting to 4");
            println!("Please use the -t or --threads flag to set number of threads");
            args.threads = 4;
        }
    } else {
        println!("\nRunning on {} threads",args.threads);
    }

    // Default to searching in start if nothing is specified
    if ( args.location.contains(&"start".into()) | args.location.contains(&"anywhere".into()) | args.location.contains(&"end".into()) ) == false {
        args.location.push(String::from("start"));
    }

    let placement = SearchPlacement {
        start: args.location.contains(&String::from("start")),
        anywhere: args.location.contains(&String::from("anywhere")),
        end: args.location.contains(&String::from("end"))
    };

    // Ensure all args are upper-case, print to user
    args.vanities.iter_mut().for_each(|s|{*s = s.to_uppercase()});
    println!("Looking for strings:\n{:?}",args.vanities);

    // Atomic boolean to keep worker threads alive
    let keep_alive = Arc::new(AtomicBool::new(true));

    // Setup communication channels between threads
    let (state_tx,state_rx) = mpsc::channel::<WorkerMsg>();
    let (print_tx,print_rx) = mpsc::channel::<PrinterMsg>();
    let (saver_tx,saver_rx) = mpsc::channel::<AddressMatch>();

    // Create and configure worker threads
    let mut worker_handles:Vec<_> = (0..args.threads).map(|id|{
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
        thread_main_loop(state_rx,saver_tx,print_tx,args.vanities.clone(),args.once,keep_alive_clone,args.threads);
        println!("Terminated thread [main_loop]")
    }));

    let keep_alive_clone = keep_alive.clone();
    worker_handles.push(thread::spawn(move||{
        thread_info_printer(print_rx, keep_alive_clone);
        println!("Terminated thread [info_printer]")
    }));

    worker_handles.push(thread::spawn(move||{
        thread_file_handler(saver_rx);
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

            // Adrress match has been found
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
                            println!("Found all vanitiy addresses!");
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
    while keep_alive.load(Ordering::Relaxed) {
        for _ in 0..COUNT_INTERVAL {
            let acc = Account::generate();
            find_vanity(&sender, &targets, &acc, &placement);
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
        println!("Speed: {} a/s",info_state.search_rate);
        println!("Total: {} million",info_state.total_count as f32 / 1e6);
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
fn thread_file_handler(receiver : mpsc::Receiver<AddressMatch>) {

    // Load existing vanity json or create a new one
    let mut matches = if let Ok(file) = File::open(DEFAULT_PATH) {
        serde_json::from_reader(&file).expect("Unable to parse existing file")
    } else {
        let file = File::create(DEFAULT_PATH).expect("Unable to create new file");
        serde_json::to_writer(&file, &[0;0]).expect("Unable to write to file");
        Vec::new()
    };

    // Receive new address match, add it to vector and save to disk
    while let Ok(message) = receiver.recv() {

        matches.push(message);
        matches.append(& mut receiver.try_iter().collect());

        if let Ok(json_message) = serde_json::to_string_pretty(&matches) {
            let mut file = File::create(DEFAULT_PATH).expect("Unable to open file as writeable");
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