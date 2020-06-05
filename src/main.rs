use serenity::{
    client::{
        bridge::gateway::GatewayIntents,
        Client, Context,
    },
    framework::standard::{
        Args, CommandResult, CheckResult, StandardFramework, Reason,
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

use sqlx::{
    Pool,
    mysql::{
        MySqlPool,
        MySqlConnection,
    }
};

use dotenv::dotenv;

use std::env;

#[group]
#[commands(help, info, autoclear, cancel_clear, rules)]
#[checks(permission_check)]
struct General;

#[check]
#[name("permission_check")]
async fn permission_check(ctx: &Context, msg: &Message) -> CheckResult {
    if let Some(guild_id) = msg.guild_id {
        if let Ok(member) = guild_id.member(ctx.clone(), msg.author.id).await {
            if let Ok(perms) = member.permissions(ctx).await {
                if perms.manage_messages() || perms.manage_guild() || perms.administrator() {
                    return CheckResult::Success
                }
            }
        }
    }

    CheckResult::Failure(Reason::User(String::from("User needs `Manage Guild` permission")))
}


struct SQLPool;

impl TypeMapKey for SQLPool {
    type Value = Pool<MySqlConnection>;
}

struct Handler;

#[serenity::async_trait]
impl EventHandler for Handler {
    async fn channel_pins_update(&self, context: Context, pin: ChannelPinsUpdateEvent) {

        let pool = context.data.read().await
            .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

        let all_ids = pin.channel_id.pins(&context).await.unwrap().iter().map(|pin| pin.id.as_u64().to_string()).collect::<Vec<String>>().join(",");

        sqlx::query!(
            "
DELETE FROM deletes WHERE message IN (?);
            ",
            all_ids
        )
            .execute(&pool)
            .await.unwrap();
    }

    async fn message(&self, ctx: Context, message: Message) {

        let c = message.channel_id;

        if let Ok(Channel::Guild(guild_channel)) = c.to_channel(&ctx).await {

            let current_user_id = ctx.cache.current_user().await.id;

            let permissions = guild_channel.permissions_for_user(&ctx, current_user_id).await.unwrap();

            if permissions.contains(Permissions::MANAGE_WEBHOOKS) && permissions.contains(Permissions::MANAGE_MESSAGES) {

                dbg!("Permissions valid");

                let user = match message.webhook_id {
                    Some(w) => w.to_webhook(&ctx).await.unwrap().user.unwrap(),

                    None => message.author,
                };

                let m = user.id.as_u64();

                let pool = ctx.data.read().await
                    .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

                let res = sqlx::query!(
                    "
SELECT timeout, message FROM channels WHERE channel = ? AND (user is null OR user = ?) AND timeout = (SELECT MIN(timeout) FROM channels WHERE channel = ? AND (user is null OR user = ?))
                    ",
                    c.as_u64(), m, c.as_u64(), m
                )
                    .fetch_one(&pool)
                    .await;

                match res {
                    Ok(row) => {
                        let mut text = Some(row.message);

                        let msg = message.id;

                        if user.bot {
                            text = None;
                        }

                        sqlx::query!(
                            "
INSERT INTO deletes (channel, message, `time`, to_send) VALUES (?, ?, ADDDATE(NOW(), INTERVAL ? SECOND), ?)
                            ",
                            c.as_u64(), msg.as_u64(), row.timeout, text
                        )
                            .execute(&pool)
                            .await.unwrap();

                    },

                    Err(_) =>
                        return (),
                }
            }
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv().ok();

    let token = env::var("DISCORD_TOKEN").expect("token");

    let user_id;

    {
        let http_client = serenity::http::client::Http::new_with_token(&format!("Bot {}", &token));

        user_id = http_client.get_current_user().await.ok().map(|current_user| current_user.id);
    }


    let framework = StandardFramework::new()
        .configure(|c| c
            .prefix("autoclear")
            .allow_dm(false)
            .ignore_bots(true)
            .ignore_webhooks(true)
            .on_mention(user_id)
        )
        .group(&GENERAL_GROUP);

    let mut client = Client::new(&env::var("DISCORD_TOKEN").expect("Missing token from environment"))
        .intents(GatewayIntents::GUILD_VOICE_STATES | GatewayIntents::GUILD_MESSAGES | GatewayIntents::GUILDS)
        .framework(framework)
        .event_handler(Handler)
        .await.expect("Error occurred creating client");

    {
        let pool = MySqlPool::new(&env::var("DATABASE_URL").expect("No database URL provided")).await.unwrap();

        let mut data = client.data.write().await;
        data.insert::<SQLPool>(pool);

    }

    client.start_autosharded().await?;

    Ok(())
}

#[command("start")]
async fn autoclear(context: &Context, message: &Message, mut args: Args) -> CommandResult {
    let mut timeout: u32 = 10;

    for arg in args.iter::<String>() {
        let a = arg.unwrap();

        if is_numeric(&a) {
            timeout = a.parse().unwrap();
            break;
        }
    }

    let to_send = args.rest();

    let msg = if to_send.is_empty() { None } else { Some(to_send) };

    let channel_id = message.channel_id;

    let pool = context.data.read().await
        .get::<SQLPool>().cloned().expect("Could not get SQLPool from data");

    let mentions = &message.mentions;

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
INSERT INTO channels (channel, timeout, message) VALUES (?, ?, ?);
            ",
            channel_id.as_u64(),
            timeout, msg
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
INSERT INTO channels (channel, user, timeout, message) VALUES (?, ?, ?, ?);
                ",
                channel_id.as_u64(), mention.id.as_u64(), timeout, msg
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

    let _ = c.send_message(context, |m| m
        .embed(|e| e
            .title("Rules")
            .description(out.join("\n"))
        )
    );

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
\t* User mentions (users the clear applies to- if no mentions, will do all users)
\t* Duration (time in seconds that messages should remain for- defaults to 10s)
\t* Message (optional, message to send when a message is deleted)

\tE.g `autoclear start @JellyWX#0001 5`

`autoclear rules` - Check the autoclear rules for specified channels. Accepts arguments:
\t* Channel mention (channel to view rules of- defaults to current)

`autoclear stop` - Cancel autoclearing on current channel. Accepts arguments:
\t* User mentions (users to cancel autoclearing for- if no mentions, will do all users)")
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
