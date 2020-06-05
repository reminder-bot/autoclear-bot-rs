CREATE TABLE autoclear.channels (
    `id` INT UNSIGNED NOT NULL AUTO_INCREMENT UNIQUE,
    `channel` BIGINT UNSIGNED NOT NULL,
    `user` BIGINT UNSIGNED DEFAULT NULL,
    `timeout` INT UNSIGNED NOT NULL DEFAULT 10,
    `message` VARCHAR(2048),

    PRIMARY KEY (id),
    UNIQUE KEY (`channel`, `user`)
);

CREATE TABLE autoclear.deletes (
    `id` INT UNSIGNED NOT NULL AUTO_INCREMENT UNIQUE,
    `channel` BIGINT UNSIGNED NOT NULL,
    `message` BIGINT UNSIGNED NOT NULL,
    `time` DATETIME NOT NULL,
    `to_send` VARCHAR(2048),

    PRIMARY KEY (id),
    UNIQUE KEY (`channel`, `message`)
);
