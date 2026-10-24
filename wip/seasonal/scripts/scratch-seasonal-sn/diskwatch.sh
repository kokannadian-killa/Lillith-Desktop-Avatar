P="zoom_tease.py (summer|newyear)_"
while [ ! -f /workspace/lillith/scratch-seasonal-sn/STOPWATCH ]; do
 f=$(df --output=avail -BG / | tail -1 | tr -dc 0-9)
 if [ $f -lt 8 ]; then pkill -STOP -f "$P"; echo "$(date +%T) paused free=${f}G" >> diskwatch.log
 elif [ $f -ge 9 ]; then pkill -CONT -f "$P"; fi
 sleep 30; done
