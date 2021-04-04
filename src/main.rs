#![allow(non_snake_case)]
use reqwest::Client;
use std::vec::Vec;

pub mod structs;
use structs::*;

pub const ENDPOINT: &'static str = "https://na1.api.riotgames.com";
pub const BLUE_SIDE: i64 = 100;
pub const RED_SIDE: i64 = 200;

/// Function expects API key to be the only thing in the file
/// Only read once, then store the string statically
fn get_api_key() -> Result<String, std::io::Error> {
    let key = std::fs::read_to_string("api.key")?.replace("\n", "");
    return Ok(key);
}

async fn get_account_info(summ_name: &str) -> Result<Account, Box<dyn std::error::Error>> {
    let slug = String::from("/lol/summoner/v4/summoners/by-name/") + summ_name +
        "?api_key=" + &get_api_key()?;
    let request = String::from(ENDPOINT) + &slug;
    println!("Sending reqwest: {}", request);
    let resp = reqwest::get(request).await?;
    let j = resp.json::<Account>().await?;
    println!("account: {:#?}", j);
    return Ok(j);
}

async fn get_matches(client: &Client, id: &str, start_idx: i64, end_idx: i64) -> Result<Matches, Box<dyn std::error::Error>> {
    let api_endpoint_base = String::from("/lol/match/v4/matchlists/by-account");
    let slug = api_endpoint_base + "/" + id +
        "?endIndex=" + &end_idx.to_string() +
        "&beginIndex=" + &start_idx.to_string() +
        "&api_key=" + &get_api_key()?;
    let request = String::from(ENDPOINT) + &slug;
    println!("Sending reqwest: {}", request);
    let resp = client.get(&request).send().await?;

    Ok(resp.json::<Matches>().await?)
}

async fn get_game_info(client: &Client, game_id: &str) -> Result<GameInfo, Box<dyn std::error::Error>> {
    let api_endpoint_base = String::from("/lol/match/v4/matches/");
    let slug = api_endpoint_base + game_id +
        "?api_key=" + &get_api_key()?;
    let request = String::from(ENDPOINT) + &slug;
    println!("Sending reqwest: {}", request);
    let resp = client.get(&request).send().await?;

    let game = match resp.json::<GameInfo>().await {
        Ok(g) => g,
        Err(e) => {
            println!("error decoding match into json: {}", e);

            // Sleep before we make the call
            std::thread::sleep(std::time::Duration::from_millis(30000));
            let resp = client.get(&request).send().await?;
            println!("response: {:#?}", resp.bytes().await?);
            let resp = client.get(&request).send().await?;
            resp.json::<GameInfo>().await?
        },
    };

    Ok(game)
}

fn data_has_game_info(data: &Vec<GameData>, game_id: i64) -> bool {
    for game_data in data {
        if game_data.game_id == game_id {
            return true;
        }
    }
    false
}

async fn collect_data(summoner: &str) -> Result<Vec<GameData>, Box<dyn std::error::Error>> {
    let mut data = Vec::<GameData>::new();

    let account_info = get_account_info(&summoner).await?;
    println!("encrypted account id : {}" , account_info.accountId);

    // See if there is a cache of this summoner
    let cache_file = String::from("cache/") + &account_info.accountId;
    if std::path::Path::new(&cache_file).exists() {
        // Load cached data
        data = serde_json::from_str(&std::fs::read_to_string(&cache_file)?)?;
    }

    // Create a reqwest Client
    let client = reqwest::Client::new();

    let mut more_matches = true;
    // The API has a limit of 100 matches at a time. Grab 100 at a time until there are no more
    let mut start_idx: i64 = 0;
    let mut end_idx: i64 = 100;
    while more_matches {
        println!("fetching matches with start_idx: {}, end_idx: {}", start_idx, end_idx);
        let matches = get_matches(&client, &account_info.accountId, start_idx, end_idx).await?;
        println!("matches: {:?}", matches);

        // Set up the next indexes
        start_idx = end_idx+1;
        end_idx = start_idx + 100;

        let range_start = matches.startIndex;
        let range_end = matches.endIndex;
        more_matches = range_end-range_start == 100;

        println!("range_start: {}, range_end: {}, diff: {}",
            range_start, range_end, range_end-range_start);
        println!("more matches: {}", more_matches);

        for a_match in matches.matches {
            let game_id = a_match.gameId;
            if data_has_game_info(&data, game_id) {
                // We already have the info for this game, skip making the request
                continue;
            }

            let mut game = GameData {
                result: GameResultData::Other,
                team: Vec::new(),
                team_of_interest: 0,
                game_id: game_id,
            };

            let game_info = get_game_info(&client, &game_id.to_string()).await?;
            // API has a quota limit, pause so we don't get an error
            // TODO - do some math and dial this down as much as possible
            std::thread::sleep(std::time::Duration::from_millis(3000));

            assert!(game_info.participantIdentities.len() == game_info.participants.len());
            let iter = game_info.participantIdentities.iter()
                .zip(game_info.participants.iter())
                .map(|(x, y)| (x, y));

            // Get all the participants for this game
            let mut blue_team: Vec<PlayerData> = Vec::new();
            let mut red_team: Vec<PlayerData> = Vec::new();
            for it in iter {
                let (participant_identity, participant) = it;

                assert!(participant.participantId == participant_identity.participantId);

                if participant_identity.player.summonerId == account_info.id {
                    game.team_of_interest = participant.teamId;
                }

                let p = PlayerData {
                    lane: participant.timeline.lane.clone(),
                    summ_name: participant_identity.player.summonerName.clone(),
                    summ_id: participant_identity.player.summonerId.clone(),

                };

                match participant.teamId {
                    BLUE_SIDE => blue_team.push(p),
                    RED_SIDE => red_team.push(p),
                    _ => panic!("Got a team id of {}", participant.teamId),
                }
            }

            for team_stats in game_info.teams {
                if team_stats.teamId == game.team_of_interest {
                    if team_stats.win == "Win" {
                        game.result = GameResultData::Win;
                    }
                }
            }

            match game.team_of_interest {
                BLUE_SIDE => game.team = blue_team,
                RED_SIDE => game.team = red_team,
                _ => panic!("team of interest is {}", game.team_of_interest),
            }

            game.game_id = game_id;

            data.push(game);
        }
    }

    println!("Writing out data to cache dir");
    let serialized = serde_json::to_string(&data)?;
    std::fs::write(cache_file, serialized)?;
    Ok(data)
}

fn analyze_data(_data: &Vec<GameData>) {
    println!("Data will be analyzed here");
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let data = collect_data("Suq Mediq").await?;
    analyze_data(&data);
    Ok(())
}
