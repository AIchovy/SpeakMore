#include "llama.h"
#include "json.hpp"
#include <iostream>
#include <string>
#include <vector>
using nlohmann::json;
int main(int argc,char **argv){
    if(argc!=2)return 2;
    if(!std::getenv("SPEAKMORE_ENGINE_DEBUG")) llama_log_set([](ggml_log_level,const char*,void*){},nullptr);
    ggml_backend_load_all();llama_backend_init();auto mp=llama_model_default_params();mp.n_gpu_layers=99;
    auto*model=llama_model_load_from_file(argv[1],mp);if(!model)return 3;
    auto cp=llama_context_default_params();cp.n_ctx=4096;cp.n_batch=2048;cp.n_threads=4;cp.n_threads_batch=4;
    auto*ctx=llama_init_from_model(model,cp);if(!ctx){llama_model_free(model);return 4;}
    const auto*vocab=llama_model_get_vocab(model);auto*sampler=llama_sampler_init_greedy();
    std::cout<<"{\"ready\":true}"<<std::endl;
    std::string line;
    while(std::getline(std::cin,line)){
        try{
            if(line.size()>32768)throw std::runtime_error("bound");auto request=json::parse(line);std::string prompt=request.at("prompt");
            int n=-llama_tokenize(vocab,prompt.c_str(),prompt.size(),nullptr,0,true,true);
            if(n<1||n>3000)throw std::runtime_error("context");std::vector<llama_token> tokens(n);llama_tokenize(vocab,prompt.c_str(),prompt.size(),tokens.data(),n,true,true);
            llama_memory_clear(llama_get_memory(ctx),true);llama_sampler_reset(sampler);
            for(int offset=0;offset<n;offset+=1024){if(llama_decode(ctx,llama_batch_get_one(tokens.data()+offset,std::min(1024,n-offset))))throw std::runtime_error("decode");}
            std::string result;bool complete=false;
            for(int i=0;i<384;++i){auto token=llama_sampler_sample(sampler,ctx,-1);if(llama_vocab_is_eog(vocab,token)){complete=true;break;}char buf[256];int len=llama_token_to_piece(vocab,token,buf,sizeof(buf),0,false);if(len<0)throw std::runtime_error("token");result.append(buf,len);if(llama_decode(ctx,llama_batch_get_one(&token,1)))throw std::runtime_error("decode");}
            std::cout<<json({{"text",result},{"complete",complete}}).dump()<<std::endl;
        }catch(...){std::cout<<"{\"error\":\"inference\"}"<<std::endl;}
    }
    llama_sampler_free(sampler);llama_free(ctx);llama_model_free(model);llama_backend_free();
}
