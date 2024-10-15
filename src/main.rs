#[macro_use]
extern crate log;
extern crate simplelog;
use log::SetLoggerError;
use serde::{Deserialize, Serialize};
use serde_json;
use simplelog::*;
use sqlx::{migrate::MigrateDatabase, Sqlite, SqlitePool};
use std::fs::File;

const DB_URL: &str = "sqlite://sqlite.db";
const SECRETS: &str = "secrets.json";
const CREDENTIALS: &str = "credentials.json";
const ENDPOINT: &str =
    "https://datacenter.disaster.go.th/apiv1/apps/minisite_datacenter/203/sitedownload/10971/23149";

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "PascalCase")]
struct FileDetail {
    #[serde(rename = "ID")]
    id: String,
    subject: String,
    link_download: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Credentials {
    token_type: String,
    scope: String,
    expires_in: i32,
    ext_expires_in: i32,
    access_token: String,
    refresh_token: String,
    id_token: String,
}

#[derive(Debug, Deserialize, Serialize)]
struct Secrets {
    tenant_id: String,
    client_id: String,
    client_secret: String,
    chat_id: String,
}

#[tokio::main]
async fn main() {
    match init_log().await {
        Ok(_) => info!("Logger initialized"),
        Err(e) => {
            error!("Error initializing logger: {}", e);
            panic!("Error initializing logger: {}", e);
        }
    };
    let db = match init_db().await {
        Ok(db) => db,
        Err(e) => {
            error!("Error initializing database: {}", e);
            panic!("Error initializing database: {}", e);
        }
    };

    match migrate_db(&db).await {
        Ok(_) => info!("Database migrated successfully"),
        Err(e) => {
            error!("Error migrating database: {}", e);
            panic!("Error migrating database: {}", e);
        }
    }
    let file_list = match fetch_json().await {
        Ok(file_list) => file_list,
        Err(e) => {
            error!("Error fetching JSON: {}", e);
            panic!("Error fetching JSON: {}", e);
        }
    };
    let new_file_list = check_new_id(file_list, &db).await;
    if new_file_list.len() == 0 {
        info!("No new files found");
        std::process::exit(0);
    }

    info!("New files found: {:?}", new_file_list);

    match refresh_token().await {
        Ok(_) => info!("Token refreshed successfully"),
        Err(e) => {
            error!("Error refreshing token: {}", e);
            panic!("Error refreshing token: {}", e);
        }
    }

    for file in new_file_list {
        match send_message(&file).await {
            Ok(_) => info!("Message sent successfully"),
            Err(e) => {
                error!("Error sending message: {}", e);
                panic!("Error sending message: {}", e);
            }
        }
    }
}

async fn init_log() -> Result<(), SetLoggerError> {
    CombinedLogger::init(vec![
        TermLogger::new(
            LevelFilter::Info,
            Config::default(),
            TerminalMode::Mixed,
            ColorChoice::Auto,
        ),
        WriteLogger::new(
            LevelFilter::Debug,
            Config::default(),
            File::create("app.log").unwrap(),
        ),
    ])
}

async fn init_db() -> Result<sqlx::Pool<Sqlite>, sqlx::Error> {
    if !Sqlite::database_exists(DB_URL).await.unwrap_or(false) {
        info!("Database does not exist, creating... {}", DB_URL);
        match Sqlite::create_database(DB_URL).await {
            Ok(_) => info!("Database created successfully"),
            Err(e) => {
                error!("Error creating database: {}", e);
                panic!("Error creating database: {}", e);
            }
        }
    } else {
        info!("Database already exists");
    }

    let db = SqlitePool::connect(DB_URL).await;

    match db {
        Ok(db) => {
            info!("Database connected successfully");
            Ok(db)
        }
        Err(e) => {
            error!("Error connecting to database: {}", e);
            Err(e)
        }
    }
}

async fn migrate_db(db: &sqlx::Pool<Sqlite>) -> Result<(), sqlx::migrate::MigrateError> {
    let migration_results = sqlx::migrate!("./migrations").run(db).await;

    match migration_results {
        Ok(_) => {
            info!("Database migrated successfully");
            Ok(())
        }
        Err(e) => {
            error!("Error migrating database: {}", e);
            Err(e)
        }
    }
}

async fn read_secrets() -> Result<Secrets, std::io::Error> {
    let file = File::open(SECRETS).expect("No secrets file found");
    let json: serde_json::Value =
        serde_json::from_reader(file).expect("Error reading secrets file");
    let secrets: Secrets = serde_json::from_value(json).expect("Cannot parse JSON");
    Ok(secrets)
}

async fn read_credentials() -> Result<Credentials, std::io::Error> {
    let file = File::open(CREDENTIALS).expect("No credentials file found");
    let json: serde_json::Value =
        serde_json::from_reader(file).expect("Error reading credentials file");
    let creds: Credentials = serde_json::from_value(json).expect("Cannot parse JSON");
    Ok(creds)
}

async fn fetch_json() -> Result<Vec<FileDetail>, reqwest::Error> {
    let client = reqwest::Client::new();
    let res = client.get(ENDPOINT).send().await?;
    let body = res.text().await?;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let data = json.get("data").expect("No Data");
    let file_list: Vec<FileDetail> =
        serde_json::from_value(data.get("file_list").expect("No File List").clone())
            .expect("Cannot parse JSON");
    debug!("{:?}", file_list);
    Ok(file_list)
}

async fn check_new_id(file_list: Vec<FileDetail>, pool: &sqlx::Pool<Sqlite>) -> Vec<FileDetail> {
    let mut new_files: Vec<FileDetail> = Vec::new();
    for file in file_list {
        let id = file.id.clone();
        let exists = query_by_id(pool, &id).await.unwrap();
        if !exists {
            info!("New file found: {}", id);
            let _ = insert_file(pool, &file).await;
            new_files.push(file);
        }
    }
    new_files.reverse();
    return new_files;
}

async fn query_by_id(pool: &sqlx::Pool<Sqlite>, id: &str) -> Result<bool, sqlx::Error> {
    let result = sqlx::query("SELECT * FROM content WHERE id = ?")
        .bind(id.parse::<i32>().unwrap_or_else(|_| 0i32))
        .fetch_optional(pool)
        .await;
    match result {
        Ok(row) => match row {
            Some(_) => Ok(true),
            None => Ok(false),
        },
        Err(e) => Err(e),
    }
}

async fn insert_file(pool: &sqlx::Pool<Sqlite>, file: &FileDetail) -> Result<(), sqlx::Error> {
    let result = sqlx::query("INSERT INTO content (id, subject, linkDownload) VALUES (?, ?, ?)")
        .bind(&file.id.parse::<i32>().unwrap_or_else(|_| 0i32))
        .bind(&file.subject)
        .bind(&file.link_download)
        .execute(pool)
        .await;
    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(e),
    }
}

async fn refresh_token() -> Result<(), reqwest::Error> {
    let secrets = read_secrets().await.unwrap();
    let creds = read_credentials().await.unwrap();
    let client = reqwest::Client::new();
    let res = client
        .post(format!(
            "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
            &secrets.tenant_id
        ))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", &secrets.client_id),
            ("client_secret", &secrets.client_secret),
            ("refresh_token", &creds.refresh_token),
            ("scope", &creds.scope),
        ])
        .send()
        .await?;
    let body = res.text().await?;
    let json: serde_json::Value = serde_json::from_str(&body).unwrap();
    let creds: Credentials = serde_json::from_value(json).unwrap();
    let file = File::create(CREDENTIALS).unwrap();
    serde_json::to_writer(file, &creds).unwrap();
    Ok(())
}

async fn send_message(file_detail: &FileDetail) -> Result<(), reqwest::Error> {
    let secrets = read_secrets().await.unwrap();
    let creds = read_credentials().await.unwrap();
    let client = reqwest::Client::new();
    let attachment_id = uuid::Uuid::new_v4().to_string();
    let res = client
        .post(format!(
            "https://graph.microsoft.com/v1.0/chats/{}/messages",
            &secrets.chat_id
        ))
        .header("Authorization", format!("Bearer {}", &creds.access_token))
        .json(&serde_json::json!({
            "body": {
                "content": format!("<attachment id=\"{}\"></attachment>", &attachment_id),
                "contentType": "html"
            },
            "attachments": [
                {
                    "id": &attachment_id,
                    "contentType": "application/vnd.microsoft.card.thumbnail",
                    "contentUrl": file_detail.link_download,
                    "name": file_detail.subject,
                    "content": serde_json::json!({
                        "title": "[New] การประกาศเขตอุทกภัย",
                        "subtitle": file_detail.subject,
                        "text": "Click the link below to download the file",
                        "buttons": [
                            {
                                "type": "openUrl",
                                "title": "Download",
                                "value": file_detail.link_download
                            }
                        ]
                    }).to_string()
                }
            ]
        }))
        .send()
        .await?;
    let body = res.text().await?;
    info!("{}", body);
    Ok(())
}
