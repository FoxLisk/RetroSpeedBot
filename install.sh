SERVICE_PATH="/lib/systemd/system/retro_speed_bot"
cp target/debug/retro_speed_bot /usr/bin/retro_speed_bot
if test -f /lib/systemd/system/retro_speed_bot.service;
then
  :
else
  cp retro_speed_bot.service $SERVICE_PATH
  echo "Please update $SERVICE_PATH with your discord token"
fi

