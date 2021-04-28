use serenity::{
    client::{
        bridge::gateway::GatewayIntents,
        Client, Context,
    },
    framework::standard::{
        Args, CommandResult, StandardFramework, Reason,
        macros::{
            command, group, check,
        }
    },
    model::{
        channel::{
            Channel,
            Message,
        },
        event::ChannelPinsUpdateEvent,
        permissions::Permissions,
    },
    prelude::{
        *
    },
};

use sqlx::mysql::MySqlPool;

use dotenv::dotenv;

use std::env;

use regex::RegexBuilder;

static REGEX_SIZE_LIMIT: usize = 4096;

#[group]
#[commands(help, info, autoclear, cancel_clear, rules)]
#[checks(permission_check)]
struct General;

#[check]
#[name("permission_check")]
async fn permission_check(ctx: &Context, msg: &Message) -> Result<(), Reason> {
    if let Some(guild_id) = msg.guild_id {
        if let Ok(member) = guild_id.member(ctx.clone(), msg.author.id).await {
            if let Ok(perms) = member.permissions(ctx).await {
                if perms.manage_messages() || perms.manage_guild() || perms.administrator() {
                    return Ok(())
                }
            }

            if let Some(roles) = member.roles(ctx).await {
                if roles
                        .iter()
                        .filter(|r| r.permissions.manage_messages() || r.permissions.manage_guild() || r.permissions.administrator() )
                        .next()
                        .is_some() {
                    return Ok(())
                }
            }
        }
    }

    let _ = msg.channel_id.say(
        ctx,
        "You must have the `Manage Messages`, `Manage Server` or `Administrator` permission to use this command. You may also need 2FA enabled").await;

    Err(Reason::User(String::from("User needs `Manage Guild` permission")))
}


struct SQLPool;

impl TypeMapKey for SQLPool {
    type Value = MySqlPool;
}

struct Handler;

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn channel_pins_update(&self, context: Context, pin: ChannelPinsUpdateEvent) {

        let pool = context.data.read().await
            .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

        for id in pin.channel_id.pins(&context)
                .await.unwrap()
                .iter()
                .map(|pin| (*pin.id.as_u64()).to_string())
                .collect::<Vec<String>>() {

            sqlx::query!(
                "
DELETE FROM deletes WHERE message = ?
                ",
                id
            )
                .execute(&pool)
                .await.unwrap();
        }
    }

    async fn message(&self, context: Context, message: Message) {

        let channel_id = message.channel_id;

        if let Ok(Channel::Guild(guild_channel)) = channel_id.to_channel(&context).await {

            let current_user_id = context.cache.current_user().await.id;

            let permissions = guild_channel.permissions_for_user(&context, current_user_id).await.unwrap();

            if permissions.contains(Permissions::MANAGE_WEBHOOKS) && permissions.contains(Permissions::MANAGE_MESSAGES) {

                let user = match message.webhook_id {
                    Some(w) => w.to_webhook(&context).await.unwrap().user.unwrap(),

                    None => message.author,
                };

                let user_id = user.id.as_u64();

                let pool = context.data.read().await
                    .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

                let res = sqlx::query!(
                    "
SELECT timeout, message, regex
    FROM channels
    WHERE channel = ? AND (user IS NULL OR user = ?)
    ORDER BY timeout
    LIMIT 1;
                    ",
                    channel_id.as_u64(), user_id
                )
                    .fetch_one(&pool)
                    .await;

                if let Ok(row) = res {

                    let mut content_matched = true;

                    if let Some(match_str) = row.regex {
                        use std::time::SystemTime;

                        let start = SystemTime::now();

                        match RegexBuilder::new(&match_str).size_limit(REGEX_SIZE_LIMIT).build() {
                            Ok(re) => {
                                if re.find(&message.content).is_none() {
                                    content_matched = false;
                                }

                                let end = SystemTime::now();

                                println!("Regex `{}` evaluated on `{}` in {}ms", match_str, message.content, end.duration_since(start).unwrap().as_millis());
                            }

                            Err(e) => {
                                println!("Regex `{}` failed to compile: {:?}", match_str, e);
                            }
                        }
                    }

                    if content_matched {
                        let msg_on_bots = env::var("MESSAGE_ON_BOTS").map_or(false, |inner| inner == "1");
                        let is_self_message = context.cache.current_user_id().await == user.id;

                        let text = if (user.bot && !msg_on_bots) || is_self_message {
                            None
                        }
                        else {
                            row.message
                        };

                        sqlx::query!(
                            "
INSERT INTO deletes (channel, message, time, to_send) VALUES (?, ?, ADDDATE(NOW(), INTERVAL ? SECOND), ?);
                            ",
                            channel_id.as_u64(), message.id.as_u64(), row.timeout, text
                        )
                            .execute(&pool)
                            .await.unwrap();
                    }
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv()?;

    let token = env::var("DISCORD_TOKEN").expect("token");

    let user_id;

    {
        let http_client = serenity::http::client::Http::new_with_token(&format!("Bot {}", &token));

        user_id = http_client.get_current_user().await.ok().map(|current_user| current_user.id);
    }

    let framework = StandardFramework::new()
        .configure(|c| c
            .prefix("autoclear ")
            .allow_dm(false)
            .ignore_bots(true)
            .ignore_webhooks(true)
            .on_mention(user_id)
        )
        .group(&GENERAL_GROUP);

    let mut client = Client::builder(token)
        .intents(GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILD_WEBHOOKS | GatewayIntents::GUILDS)
        .framework(framework)
        .event_handler(Handler)
        .await.expect("Error occurred creating client");

    {
        let pool = MySqlPool::connect(&env::var("DATABASE_URL").expect("No database URL provided")).await.unwrap();

        let mut data = client.data.write().await;
        data.insert::<SQLPool>(pool);
    }

    client.start_autosharded().await?;

    Ok(())
}

#[command("start")]
async fn autoclear(context: &Context, message: &Message, mut args: Args) -> CommandResult {
    #[derive(PartialEq)]
    enum NamedArg {
        NotProvided,
        Next,
        Provided(String)
    }

    impl NamedArg {
        fn ok(&self) -> Option<String> {
            match self {
                NamedArg::Provided(val) => {
                    Some(val.to_string())
                }
                _ => {
                    None
                }
            }
        }
    }

    let mut timeout: u32 = 10;

    let mut regex: NamedArg = NamedArg::NotProvided;
    let mut to_send: NamedArg = NamedArg::NotProvided;

    for arg_res in args.iter::<String>() {
        let arg = arg_res.unwrap().trim_matches('"').to_string();

        if is_numeric(&arg) {
            timeout = arg.parse().unwrap();
        }
        else if regex == NamedArg::Next {
            regex = NamedArg::Provided(arg);
        }
        else if to_send == NamedArg::Next {
            to_send = NamedArg::Provided(arg);
        }
        else {
            match arg.as_str() {
                "-r" | "--regex" => {
                    regex = NamedArg::Next;
                }

                "-m" | "--message" => {
                    to_send = NamedArg::Next;
                }

                _ => {}
            }
        }
    }

    let channel_id = message.channel_id;

    let pool = context.data.read().await
        .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

    let mentions = &message.mentions;

    if let Some(match_str) = regex.ok() {
        if match_str.len() > 64 {
            message.reply(context, "Regex too long: regex must not exceed 64 characters in length").await?;

            return Ok(())
        }
        else if RegexBuilder::new(&match_str).size_limit(REGEX_SIZE_LIMIT).build().is_err() {
            message.reply(context, format!("Compiled regex too long: compiled regex must not exceed {} bytes in length", REGEX_SIZE_LIMIT)).await?;

            return Ok(())
        }
    }

    if mentions.len() == 0 {
        sqlx::query!(
            "
DELETE FROM channels WHERE channel = ? AND user IS NULL;
            ",
            channel_id.as_u64(),
        )
            .execute(&pool)
            .await?;

        sqlx::query!(
            "
INSERT INTO channels (channel, timeout, message, regex) VALUES (?, ?, ?, ?);
            ",
            channel_id.as_u64(),
            timeout, to_send.ok(), regex.ok()
        )
            .execute(&pool)
            .await?;

        message.reply(context, "Autoclearing channel.").await?;
    }
    else {
        for mention in mentions {
            sqlx::query!(
                "
DELETE FROM channels WHERE channel = ? AND user = ?;
                ",
                channel_id.as_u64(), mention.id.as_u64()
            )
                .execute(&pool)
                .await?;

            sqlx::query!(
                "
INSERT INTO channels (channel, user, timeout, message, regex) VALUES (?, ?, ?, ?, ?);
                ",
                channel_id.as_u64(), mention.id.as_u64(), timeout, to_send.ok(), regex.ok()
            )
                .execute(&pool)
                .await?;
        }

        message.reply(context, &format!("Autoclearing {} users.", mentions.len())).await?;
    }

    Ok(())
}


#[command("stop")]
async fn cancel_clear(context: &Context, message: &Message) -> CommandResult {
    let channel_id = message.channel_id;

    let pool = context.data.read().await
        .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

    let mentions = &message.mentions;

    if mentions.len() == 0 {
        sqlx::query!(
            "
DELETE FROM channels WHERE channel = ? AND user IS NULL
            ",
            channel_id.as_u64()
        )
            .execute(&pool)
            .await?;

        message.reply(context, "Global autoclear cancelled on this channel.").await?;
    }
    else {
        let joined_mentions = mentions.iter().map(|m| m.id.as_u64().to_string()).collect::<Vec<String>>().join(", ");
        sqlx::query!(
            "
DELETE FROM channels WHERE channel = ? AND user IN (?);
            ",
            channel_id.as_u64(), joined_mentions
        )
            .execute(&pool)
            .await?;

        message.reply(context, &format!("Autoclear cancelled on {} users.", mentions.len())).await?;
    }

    Ok(())
}


#[command("rules")]
async fn rules(context: &Context, message: &Message) -> CommandResult {
    let c = message.channel_id;

    let mut out: Vec<String> = vec![];

    {
        let pool = context.data.read().await
            .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

        let res = sqlx::query!(
            "
SELECT user, timeout FROM channels WHERE channel = ?;
            ",
            c.as_u64()
        )
            .fetch_all(&pool)
            .await?;

        for row in res {

            match row.user {
                Some(u) => {
                    out.push(format!("**<@{}>**: {}s", u, row.timeout));
                },

                None => {
                    out.insert(0, format!("**GLOBAL**: {}s", row.timeout));
                },
            }
        }
    }

    c.send_message(context, |m| m
        .embed(|e| e
            .title("Rules")
            .description(out.join("\n"))
        )
    ).await?;

    Ok(())
}


fn is_numeric(s: &String) -> bool {
    let m = s.matches(char::is_numeric);

    if m.into_iter().count() == s.len() {
        return true
    }
    false
}


#[command]
async fn help(context: &Context, message: &Message) -> CommandResult {
    message.channel_id.send_message(context, |m| {
        m.embed(|e| {
            e.title("Help")
            .description("`autoclear start` - Start autoclearing the current channel. Accepts arguments:
* User mentions (users the clear applies to- if no mentions, will do all users)
* Duration (time in seconds that messages should remain for- defaults to 10s)
* Message (optional quoted with named specifier `-m`, message to send when a message is deleted)
* Regex (optional quoted with named specifier `-r`, regex to use to evaluate which messages to delete. Max 64 characters, 4KB compiled)

E.g `autoclear start @JellyWX#0001 5`
*Using a message*
`autoclear start 300 -m \"Message deleted after 5 minutes\"`
*Using a regex to clear up links*
`autoclear start 1 -r \"(http://)|(https://)\" -m \"Links are banned in this channel\"`

`autoclear rules` - Check the autoclear rules for specified channels. Accepts arguments:
* Channel mention (channel to view rules of- defaults to current)

`autoclear stop` - Cancel autoclearing on current channel. Accepts arguments:
* User mentions (users to cancel autoclearing for- if no mentions, will do all users)")
        })
    }).await?;

    Ok(())
}


#[command]
async fn info(context: &Context, message: &Message) -> CommandResult {
    message.channel_id.send_message(context, |m| {
        m.embed(|e| {
            e.title("Info")
            .description("
Invite me: https://discordapp.com/oauth2/authorize?client_id=488060245739044896&scope=bot&permissions=93184

Join the Discord server: https://discord.jellywx.com/

Do `autoclear help` for more.
            ")
        })
    }).await?;

    Ok(())
}
