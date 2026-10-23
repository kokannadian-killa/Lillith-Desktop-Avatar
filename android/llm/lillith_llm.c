// JNI shim: com.kokannadian.lillith.LocalLlm -> llama.cpp (on device).
//
// HARD RULE: fully local. The GGUF path comes from the user's own file pick;
// no network, no downloads, no telemetry. Replies are returned as UTF-8
// bytes (JNI's NewStringUTF can't carry 4-byte UTF-8 such as emoji).
#include <jni.h>
#include <stdlib.h>
#include <string.h>
#include <android/log.h>

#include "llama.h"

#define TAG "LillithLLM"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, TAG, __VA_ARGS__)
#define LOGE(...) __android_log_print(ANDROID_LOG_ERROR, TAG, __VA_ARGS__)

typedef struct {
    struct llama_model *model;
    struct llama_context *ctx;
    int n_batch;
} Llm;

static void quiet_log(enum ggml_log_level level, const char *text, void *ud) {
    (void)ud;
    if (level >= GGML_LOG_LEVEL_WARN) __android_log_print(ANDROID_LOG_WARN, TAG, "%s", text);
}

JNIEXPORT jlong JNICALL
Java_com_kokannadian_lillith_LocalLlm_nLoad(JNIEnv *env, jclass cls, jstring jpath, jint n_ctx,
                                            jint n_threads) {
    (void)cls;
    static int inited = 0;
    if (!inited) {
        llama_log_set(quiet_log, NULL);
        llama_backend_init();
        inited = 1;
    }
    const char *path = (*env)->GetStringUTFChars(env, jpath, NULL);
    struct llama_model_params mp = llama_model_default_params();
    mp.n_gpu_layers = 0;
    struct llama_model *model = llama_model_load_from_file(path, mp);
    (*env)->ReleaseStringUTFChars(env, jpath, path);
    if (!model) {
        LOGE("model load failed");
        return 0;
    }
    struct llama_context_params cp = llama_context_default_params();
    cp.n_ctx = n_ctx > 0 ? (uint32_t)n_ctx : 2048;
    cp.n_batch = 512;
    cp.n_threads = n_threads > 0 ? n_threads : 4;
    cp.n_threads_batch = cp.n_threads;
    struct llama_context *ctx = llama_init_from_model(model, cp);
    if (!ctx) {
        LOGE("context init failed");
        llama_model_free(model);
        return 0;
    }
    Llm *l = calloc(1, sizeof(Llm));
    l->model = model;
    l->ctx = ctx;
    l->n_batch = 512;
    LOGI("model loaded (n_ctx=%u)", llama_n_ctx(ctx));
    return (jlong)(intptr_t)l;
}

JNIEXPORT void JNICALL
Java_com_kokannadian_lillith_LocalLlm_nFree(JNIEnv *env, jclass cls, jlong h) {
    (void)env;
    (void)cls;
    Llm *l = (Llm *)(intptr_t)h;
    if (!l) return;
    llama_free(l->ctx);
    llama_model_free(l->model);
    free(l);
}

static char *dup_jstring(JNIEnv *env, jstring s) {
    if (!s) return strdup("");
    const char *c = (*env)->GetStringUTFChars(env, s, NULL);
    char *out = strdup(c);
    (*env)->ReleaseStringUTFChars(env, s, c);
    return out;
}

// roles/contents: the chat so far (system first). Returns the reply bytes.
JNIEXPORT jbyteArray JNICALL
Java_com_kokannadian_lillith_LocalLlm_nChat(JNIEnv *env, jclass cls, jlong h, jobjectArray roles,
                                            jobjectArray contents, jint max_tokens, jfloat temp,
                                            jfloat top_p, jint seed) {
    (void)cls;
    Llm *l = (Llm *)(intptr_t)h;
    if (!l) return NULL;
    const struct llama_vocab *vocab = llama_model_get_vocab(l->model);
    jsize n = (*env)->GetArrayLength(env, roles);
    struct llama_chat_message *msgs = calloc((size_t)n, sizeof(*msgs));
    size_t total = 0;
    for (jsize i = 0; i < n; i++) {
        msgs[i].role = dup_jstring(env, (jstring)(*env)->GetObjectArrayElement(env, roles, i));
        msgs[i].content = dup_jstring(env, (jstring)(*env)->GetObjectArrayElement(env, contents, i));
        total += strlen(msgs[i].content) + 64;
    }
    const char *tmpl = llama_model_chat_template(l->model, NULL);
    if (!tmpl) tmpl = "chatml";
    int32_t cap = (int32_t)(total * 2 + 256);
    char *prompt = malloc((size_t)cap);
    int32_t len = llama_chat_apply_template(tmpl, msgs, (size_t)n, true, prompt, cap);
    if (len > cap) {
        prompt = realloc(prompt, (size_t)len + 1);
        len = llama_chat_apply_template(tmpl, msgs, (size_t)n, true, prompt, len + 1);
    }
    for (jsize i = 0; i < n; i++) {
        free((void *)msgs[i].role);
        free((void *)msgs[i].content);
    }
    free(msgs);
    if (len < 0) {
        free(prompt);
        LOGE("chat template failed");
        return NULL;
    }
    prompt[len] = 0;

    int32_t n_tok = -llama_tokenize(vocab, prompt, len, NULL, 0, true, true);
    llama_token *toks = malloc(sizeof(llama_token) * (size_t)(n_tok > 0 ? n_tok : 1));
    n_tok = llama_tokenize(vocab, prompt, len, toks, n_tok, true, true);
    free(prompt);
    int32_t n_ctx = (int32_t)llama_n_ctx(l->ctx);
    if (max_tokens <= 0) max_tokens = 256;
    if (n_tok <= 0 || n_tok + max_tokens >= n_ctx) {
        // Keep the tail of the prompt (the newest turns) if it is too long.
        int32_t keep = n_ctx - max_tokens - 8;
        if (n_tok <= 0 || keep <= 16) {
            free(toks);
            LOGE("prompt does not fit (%d tokens, n_ctx %d)", n_tok, n_ctx);
            return NULL;
        }
        memmove(toks, toks + (n_tok - keep), sizeof(llama_token) * (size_t)keep);
        n_tok = keep;
    }

    llama_memory_clear(llama_get_memory(l->ctx), true);
    for (int32_t i = 0; i < n_tok; i += l->n_batch) {
        int32_t cnt = n_tok - i < l->n_batch ? n_tok - i : l->n_batch;
        if (llama_decode(l->ctx, llama_batch_get_one(toks + i, cnt)) != 0) {
            free(toks);
            LOGE("decode failed");
            return NULL;
        }
    }
    free(toks);

    struct llama_sampler *smpl = llama_sampler_chain_init(llama_sampler_chain_default_params());
    llama_sampler_chain_add(smpl, llama_sampler_init_top_p(top_p > 0 ? top_p : 0.9f, 1));
    llama_sampler_chain_add(smpl, llama_sampler_init_temp(temp > 0 ? temp : 0.8f));
    llama_sampler_chain_add(smpl, llama_sampler_init_dist((uint32_t)seed));

    size_t out_cap = 4096, out_len = 0;
    char *out = malloc(out_cap);
    for (int i = 0; i < max_tokens; i++) {
        llama_token t = llama_sampler_sample(smpl, l->ctx, -1);
        if (llama_vocab_is_eog(vocab, t)) break;
        char piece[256];
        int32_t pn = llama_token_to_piece(vocab, t, piece, sizeof(piece), 0, false);
        if (pn > 0) {
            if (out_len + (size_t)pn >= out_cap) {
                out_cap *= 2;
                out = realloc(out, out_cap);
            }
            memcpy(out + out_len, piece, (size_t)pn);
            out_len += (size_t)pn;
        }
        llama_token one = t;
        if (llama_decode(l->ctx, llama_batch_get_one(&one, 1)) != 0) break;
    }
    llama_sampler_free(smpl);
    jbyteArray arr = (*env)->NewByteArray(env, (jsize)out_len);
    (*env)->SetByteArrayRegion(env, arr, 0, (jsize)out_len, (const jbyte *)out);
    free(out);
    return arr;
}
