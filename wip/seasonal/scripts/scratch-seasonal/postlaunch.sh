while kill -0  2>/dev/null; do sleep 15; done
echo halloween_cape_swirl_arch >> post_done.log
nohup bash postwatch2.sh > postwatch2.log 2>&1 &
