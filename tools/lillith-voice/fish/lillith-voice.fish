# Lillith offline voice - fish integration (installed to ~/.config/fish/conf.d/lillith-voice.fish)
fish_add_path -g $HOME/.local/bin

# Speak text / stdin in Lillith's voice:  say "hey babe"   |   echo hi | say
function say --wraps lillith-say --description "Lillith says it (offline, her cloned voice)"
    lillith-say $argv
end

# Preset shortcuts (default = lillith, Ko's cloned voice)
function say-fast   --description "Lillith voice: lillith_fast (int8, for slow CPUs)"; lillith-say -p lillith_fast $argv; end
function say-p1     --description "Lillith voice: p1 (Kokoro bella, F0 290 Hz)";     lillith-say -p p1 $argv; end
function say-bella  --description "Lillith voice: bella (old default)";               lillith-say -p bella $argv; end
function say-heart  --description "Lillith voice: bella85_heart15 (sweeter)";   lillith-say -p bella85_heart15 $argv; end
function say-nicole --description "Lillith voice: bella80_nicole20 (whispery)"; lillith-say -p bella80_nicole20 $argv; end
function say-sky    --description "Lillith voice: bella75_sky25 (lower, lazier)"; lillith-say -p bella75_sky25 $argv; end

# Shut her up mid-sentence
function hush --description "Stop Lillith talking"
    lillith-say --stop
end

# Talk to the local llama-server (127.0.0.1:8080) and hear the answer
function lil --wraps lillith-chat --description "Chat with Lillith via local llama-server, spoken"
    lillith-chat $argv
end

# Talk to her with the mic (push-to-talk: Enter / Enter), or hands-free with: lil --mic
function lil-talk --description "Voice chat with Lillith (push-to-talk)"
    lillith-chat --ptt $argv
end
# Speech-to-text only: hear  (one utterance)   |   hear --setup  (pick + test the mic)
function hear --wraps lillith-listen --description "Transcribe what you say (local whisper)"
    lillith-listen $argv
end

# Daemon control (systemd --user unit, falls back to a background process)
function lillith-voice-start --description "Start Lillith's voice daemon"
    if systemctl --user cat lillith-voice.service >/dev/null 2>&1
        systemctl --user start lillith-voice.service
    else
        lillith-voiced >/dev/null 2>&1 &
        disown
    end
end
function lillith-voice-stop --description "Stop Lillith's voice daemon"
    if systemctl --user is-active -q lillith-voice.service
        systemctl --user stop lillith-voice.service
    else
        pkill -f "lillith_voice.daemon"
    end
end
