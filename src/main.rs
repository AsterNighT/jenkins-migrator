extern crate clap;
extern crate quick_xml;
use clap::{App, Arg};
use quick_xml::{events::BytesText, Reader};
use quick_xml::{events::Event, Writer};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Cursor, Read, Write},
};
fn main() {
    // let matches = App::new("Migrator")
    //     .version("1.0")
    //     .author("AsterNighT <klxjt99@outlook.com>")
    //     .about("F jenkins hardcoded pipeline script to local/git repo and make it fetch from scm")
    //     .arg(
    //         Arg::with_name("config")
    //             .short("c")
    //             .long("config")
    //             .value_name("FILE")
    //             .required(true)
    //             .help("Sets a custom config file")
    //             .takes_value(true),
    //     )
    //     .get_matches();

    // // Gets a value for config if supplied by user, or defaults to "default.conf"
    // let config = matches.value_of("config").unwrap_or("default.conf");
    let config = "config.json";
    println!("Path for config: {}", config);
    run_with_config_path(config).expect("Failed to do the job");
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Job {
    #[serde(default)]
    script_base_path: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    rename_to: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    jenkins_url: String,
    #[serde(default)]
    github_repo: String,
    #[serde(default)]
    branch_specifier: String,
    #[serde(default)]
    jenkins_user: String,
    #[serde(default)]
    jenkins_token: String,
    #[serde(default)]
    push_to_jenkins: bool,
    #[serde(default)]
    fetch_from_jenkins: bool,
    #[serde(default)]
    jobs: Vec<Job>,
}

fn run_with_config_path(config: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut file = File::open(config)?;
    let mut contents = String::new();
    file.read_to_string(&mut contents)?;
    let config: Config = serde_json::from_str(&contents)?;
    let client = reqwest::blocking::Client::new();
    for job in config.jobs.iter() {
        let name = if job.rename_to.is_empty() {
            &job.name
        } else {
            &job.rename_to
        };
        let name = name.clone() + ".groovy";
        println!("{}", name);
        if config.fetch_from_jenkins {
            let url = format!("{}/job/{}/config.xml", config.jenkins_url, job.name);
            let resp = client
                .get(&url)
                .basic_auth(&config.jenkins_user, Some(&config.jenkins_token))
                .send()
                .expect("Get failed");
            println!("{:#?}", resp);
            let text = resp.text().expect("Failed to parse response");
            let mut reader = Reader::from_str(&text);
            reader.trim_text(true);
            let mut in_script = false;
            let mut script = String::new();
            let mut buf = Vec::new();

            loop {
                match reader.read_event(&mut buf) {
                    Ok(Event::Start(ref e)) => match e.name() {
                        b"script" => in_script = true,
                        _ => (),
                    },
                    Ok(Event::End(ref e)) => match e.name() {
                        b"script" => in_script = false,
                        _ => (),
                    },
                    Ok(Event::Text(e)) if in_script => {
                        script = e.unescape_and_decode(&reader).unwrap();
                        break;
                    }
                    Ok(Event::Eof) => break, // exits the loop when reaching end of file
                    Err(e) => panic!("Error at position {}: {:?}", reader.buffer_position(), e),
                    _ => (), // There are several other `Event`s we do not consider here
                }

                // if we don't keep a borrow elsewhere, we can clear the buffer to keep memory usage low
                buf.clear();
            }
            let path = format!("{}{}", job.script_base_path, name);
            std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(path)?
                .write_all(script.as_bytes())?;
        }
        if config.push_to_jenkins {
            let url = format!("{}/job/{}/config.xml", config.jenkins_url, job.name);
            let resp = client
                .get(&url)
                .basic_auth(&config.jenkins_user, Some(&config.jenkins_token))
                .send()
                .expect("Get failed");
            println!("{:#?}", resp);
            let text = resp.text().expect("Failed to parse response");
            let mut reader = Reader::from_str(&text);
            reader.trim_text(true);
            let mut writer = Writer::new(Cursor::new(Vec::new()));
            let mut buf = Vec::new();
            let mut in_script = false;
            loop {
                match reader.read_event(&mut buf) {
                    Ok(Event::Start(ref e)) if e.name() == b"definition" => {
                        in_script = true;
                        let mut elem =
                            quick_xml::events::BytesStart::owned(b"definition".to_vec(), "definition".len());
                        // copy existing attributes, adds a new my-key="some value" attribute
                        elem.push_attribute(("class", "org.jenkinsci.plugins.workflow.cps.CpsScmFlowDefinition"));
                        elem.push_attribute(("plugin","workflow-cps@2.70"));
                        assert!(writer.write_event(Event::Start(elem)).is_ok());
                    }
                    Ok(Event::End(ref e)) if e.name() == b"definition" => {
                        in_script = false;
                        assert!(writer.write_event(Event::End(e.clone())).is_ok());
                    }
                    Ok(_) if in_script => {
                        continue;
                    }
                    Ok(Event::Eof) => break,
                    // we can either move or borrow the event to write, depending on your use-case
                    Ok(e) => assert!(writer.write_event(&e).is_ok()),
                    Err(e) => panic!("{}", e),
                }
                buf.clear();
            }
            let raw = writer.into_inner().into_inner();
            let result = std::str::from_utf8(&raw).expect("");
            let nodes = format!(
                r###"<scm class="hudson.plugins.git.GitSCM" plugin="git@3.10.0"><configVersion>2</configVersion><userRemoteConfigs><hudson.plugins.git.UserRemoteConfig><url>{}</url></hudson.plugins.git.UserRemoteConfig></userRemoteConfigs><branches><hudson.plugins.git.BranchSpec><name>{}</name></hudson.plugins.git.BranchSpec></branches><doGenerateSubmoduleConfigurations>false</doGenerateSubmoduleConfigurations><submoduleCfg class="list"/><extensions/></scm><scriptPath>{}/{}</scriptPath><lightweight>true</lightweight>"###,
                config.github_repo, config.branch_specifier, job.script_base_path, name
            );
            println!("Generated text: {}", nodes);
            let result = result.replace("</definition>", &(nodes + "</definition>"));
            println!("{}", result);
            let resp = client
                .post(&url)
                .basic_auth(&config.jenkins_user, Some(&config.jenkins_token))
                .body(result)
                .send();
            println!("{:#?}", resp);
        }
    }
    Ok(())
}
