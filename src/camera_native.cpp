// Camera-only DirectShow bridge. No USB sharing changes and no image persistence.
#include <windows.h>
#include <dshow.h>
#include <dvdmedia.h>
#include <setupapi.h>
#include <cfgmgr32.h>
#include <wrl/client.h>
#include <algorithm>
#include <atomic>
#include <chrono>
#include <cstdint>
#include <cwchar>
#include <memory>
#include <mutex>
#include <string>
#include <vector>
using Microsoft::WRL::ComPtr;
namespace {
constexpr size_t max_frame_bytes=64*1024*1024;
struct Failure { HRESULT value; };
void check(HRESULT result) { if(FAILED(result))throw Failure{result}; }
struct Apartment {
    Apartment(){check(CoInitializeEx(nullptr,COINIT_MULTITHREADED));}
    ~Apartment(){CoUninitialize();}
};
struct MediaType {
    AM_MEDIA_TYPE* value=nullptr;
    MediaType()=default;
    MediaType(const MediaType&)=delete;
    MediaType& operator=(const MediaType&)=delete;
    ~MediaType(){if(value){if(value->cbFormat)CoTaskMemFree(value->pbFormat);if(value->pUnk)value->pUnk->Release();CoTaskMemFree(value);}}
};
struct PropertyValue {
    VARIANT value;
    PropertyValue(){VariantInit(&value);}
    ~PropertyValue(){VariantClear(&value);}
    PropertyValue(const PropertyValue&)=delete;
    PropertyValue& operator=(const PropertyValue&)=delete;
};
struct DeviceInfoSet {
    HDEVINFO value=SetupDiCreateDeviceInfoList(nullptr,nullptr);
    ~DeviceInfoSet(){if(value!=INVALID_HANDLE_VALUE)SetupDiDestroyDeviceInfoList(value);}
    DeviceInfoSet()=default;
    DeviceInfoSet(const DeviceInfoSet&)=delete;
    DeviceInfoSet& operator=(const DeviceInfoSet&)=delete;
};
std::wstring quote(const std::wstring& value) {
    std::wstring out=L"\"";
    for(wchar_t ch:value){
        if(ch==L'"'||ch==L'\\'){out+=L'\\';out+=ch;}
        else if(ch<32){wchar_t escaped[8];swprintf_s(escaped,L"\\u%04x",static_cast<unsigned>(ch));out+=escaped;}
        else out+=ch;
    }
    return out+L'"';
}
void result_string(const std::wstring& value,wchar_t** output) {
    auto* buffer=static_cast<wchar_t*>(CoTaskMemAlloc((value.size()+1)*sizeof(wchar_t)));
    if(!buffer)throw std::bad_alloc();
    memcpy(buffer,value.c_str(),(value.size()+1)*sizeof(wchar_t));*output=buffer;
}
std::wstring property(IPropertyBag* bag,const wchar_t* key) {
    PropertyValue owned;auto& value=owned.value;
    if(SUCCEEDED(bag->Read(key,&value,nullptr))&&value.vt==VT_BSTR&&value.bstrVal)return value.bstrVal;
    return {};
}
// Pure DirectShow sources (OBS/Unity Capture) need not have a PnP DevicePath.
std::wstring camera_path(IMoniker* moniker,IPropertyBag* bag) {
    auto path=property(bag,L"DevicePath");if(!path.empty())return path;
    LPOLESTR display=nullptr;
    if(SUCCEEDED(moniker->GetDisplayName(nullptr,nullptr,&display))&&display){path=display;CoTaskMemFree(display);}
    return path;
}
std::wstring identities(const std::wstring& path) {
    std::wstring output=L"[";bool first=true;
    DeviceInfoSet owned;HDEVINFO devices=owned.value;
    if(devices==INVALID_HANDLE_VALUE)return L"[]";
    SP_DEVICE_INTERFACE_DATA interface_data{};interface_data.cbSize=sizeof(interface_data);
    if(SetupDiOpenDeviceInterfaceW(devices,path.c_str(),0,&interface_data)){
        DWORD size=0;SetupDiGetDeviceInterfaceDetailW(devices,&interface_data,nullptr,0,&size,nullptr);
        if(size>=sizeof(SP_DEVICE_INTERFACE_DETAIL_DATA_W)&&size<=65536){
            std::vector<BYTE> buffer(size);auto* detail=reinterpret_cast<SP_DEVICE_INTERFACE_DETAIL_DATA_W*>(buffer.data());detail->cbSize=sizeof(*detail);
            SP_DEVINFO_DATA node{};node.cbSize=sizeof(node);
            if(SetupDiGetDeviceInterfaceDetailW(devices,&interface_data,detail,size,nullptr,&node)){
                DEVINST current=node.DevInst;
                for(int depth=0;depth<16;++depth){
                    wchar_t id[MAX_DEVICE_ID_LEN]{};
                    if(CM_Get_Device_IDW(current,id,MAX_DEVICE_ID_LEN,0)!=CR_SUCCESS)break;
                    if(!first)output+=L",";first=false;output+=quote(id);
                    DEVINST parent=0;if(CM_Get_Parent(&parent,current,0)!=CR_SUCCESS)break;current=parent;
                }
            }
        }
    }
    return output+L"]";
}
ComPtr<IEnumMoniker> monikers() {
    ComPtr<ICreateDevEnum> source;check(CoCreateInstance(CLSID_SystemDeviceEnum,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&source)));
    ComPtr<IEnumMoniker> list;check(source->CreateClassEnumerator(CLSID_VideoInputDeviceCategory,&list,0));return list;
}
ComPtr<IBaseFilter> camera(const wchar_t* path) {
    auto list=monikers();if(!list)throw Failure{HRESULT_FROM_WIN32(ERROR_NOT_FOUND)};
    for(ComPtr<IMoniker> moniker;list->Next(1,&moniker,nullptr)==S_OK;moniker.Reset()){
        ComPtr<IPropertyBag> bag;if(FAILED(moniker->BindToStorage(nullptr,nullptr,IID_PPV_ARGS(&bag))))continue;
        if(_wcsicmp(camera_path(moniker.Get(),bag.Get()).c_str(),path)==0){
            ComPtr<IBaseFilter> filter;
            if(_wcsnicmp(path,L"\\\\?\\usb#",8)==0){
                // Use the Windows WDM proxy for USB capture. The per-device MF bridge
                // can stay indefinitely intermediate with no samples (e.g. Chicony B729).
                // Load only this enumerated device's property bag; no registry/policy edits.
                const CLSID proxy={0x17CCA71B,0xECD7,0x11D0,{0xB9,0x08,0x00,0xA0,0xC9,0x22,0x31,0x96}};
                check(CoCreateInstance(proxy,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&filter)));
                ComPtr<IPersistPropertyBag> persist;check(filter.As(&persist));check(persist->Load(bag.Get(),nullptr));
            }else check(moniker->BindToObject(nullptr,nullptr,IID_PPV_ARGS(&filter)));
            return filter;
        }
    }
    throw Failure{HRESULT_FROM_WIN32(ERROR_NOT_FOUND)};
}
struct CaptureSource {
    ComPtr<IBaseFilter> device;
    ComPtr<IGraphBuilder> graph;
    ComPtr<ICaptureGraphBuilder2> builder;
    ComPtr<IAMStreamConfig> config;
    explicit CaptureSource(const wchar_t* path):device(camera(path)) {
        check(CoCreateInstance(CLSID_FilterGraph,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&graph)));
        check(CoCreateInstance(CLSID_CaptureGraphBuilder2,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&builder)));
        check(builder->SetFiltergraph(graph.Get()));check(graph->AddFilter(device.Get(),L"Camera"));
        check(builder->FindInterface(&PIN_CATEGORY_CAPTURE,&MEDIATYPE_Video,device.Get(),IID_IAMStreamConfig,reinterpret_cast<void**>(config.GetAddressOf())));
    }
};
struct VideoType { LONG width=0,height=0;REFERENCE_TIME interval=0;bool top_down=false; };
VideoType video_type(const AM_MEDIA_TYPE& type) {
    VideoType out;
    if(type.formattype==FORMAT_VideoInfo&&type.pbFormat&&type.cbFormat>=sizeof(VIDEOINFOHEADER)){
        const auto* info=reinterpret_cast<const VIDEOINFOHEADER*>(type.pbFormat);out={info->bmiHeader.biWidth,info->bmiHeader.biHeight,info->AvgTimePerFrame};
    }
    if(type.formattype==FORMAT_VideoInfo2&&type.pbFormat&&type.cbFormat>=sizeof(VIDEOINFOHEADER2)){
        const auto* info=reinterpret_cast<const VIDEOINFOHEADER2*>(type.pbFormat);out={info->bmiHeader.biWidth,info->bmiHeader.biHeight,info->AvgTimePerFrame};
    }
    if(out.height<0&&out.height>=-8192){out.top_down=true;out.height=-out.height;}
    return out;
}
bool bounded(const VideoType& video) {
    return video.width>0&&video.width<=8192&&video.height>0&&video.height<=8192&&video.interval>0
        &&static_cast<uint64_t>(video.width)*video.height*4<=max_frame_bytes;
}
std::wstring subtype_name(const GUID& subtype) {
    if(subtype==MEDIASUBTYPE_MJPG)return L"MJPEG";
    if(subtype==MEDIASUBTYPE_YUY2)return L"YUY2";
    if(subtype==MEDIASUBTYPE_RGB32)return L"RGB32";
    if(subtype==MEDIASUBTYPE_RGB24)return L"RGB24";
    wchar_t guid[40]{};StringFromGUID2(subtype,guid,40);return guid;
}
std::wstring mode_key(const AM_MEDIA_TYPE& type) {
    auto video=video_type(type);wchar_t guid[40]{};StringFromGUID2(type.subtype,guid,40);
    return std::wstring(guid)+L":"+std::to_wstring(video.width)+L":"+std::to_wstring(video.height)+L":"+std::to_wstring(video.interval);
}
MIDL_INTERFACE("0579154A-2B53-4994-B0D0-E773148EFF85") ISampleGrabberCB:public IUnknown {
    virtual HRESULT STDMETHODCALLTYPE SampleCB(double,IMediaSample*)=0;
    virtual HRESULT STDMETHODCALLTYPE BufferCB(double,BYTE*,long)=0;
};
MIDL_INTERFACE("6B652FFF-11FE-4FCE-92AD-0266B5D7C78F") ISampleGrabber:public IUnknown {
    virtual HRESULT STDMETHODCALLTYPE SetOneShot(BOOL)=0;
    virtual HRESULT STDMETHODCALLTYPE SetMediaType(const AM_MEDIA_TYPE*)=0;
    virtual HRESULT STDMETHODCALLTYPE GetConnectedMediaType(AM_MEDIA_TYPE*)=0;
    virtual HRESULT STDMETHODCALLTYPE SetBufferSamples(BOOL)=0;
    virtual HRESULT STDMETHODCALLTYPE GetCurrentBuffer(long*,long*)=0;
    virtual HRESULT STDMETHODCALLTYPE GetCurrentSample(IMediaSample**)=0;
    virtual HRESULT STDMETHODCALLTYPE SetCallback(ISampleGrabberCB*,long)=0;
};
const GUID grabber_class={0xC1F400A0,0x3F08,0x11D3,{0x9F,0x0B,0x00,0x60,0x08,0x03,0x9E,0x37}};
const GUID null_class={0xC1F400A4,0x3F08,0x11D3,{0x9F,0x0B,0x00,0x60,0x08,0x03,0x9E,0x37}};
struct Shared {
    std::atomic<uint64_t> input_frames{0},input_bytes{0},decoded_frames{0};
    std::mutex mutex;std::vector<uint8_t> frame;uint64_t sequence=0;
    size_t frame_bytes=0;LONG width=0,height=0;bool top_down=false;
};
class Samples final:public ISampleGrabberCB {
    std::atomic<ULONG> references{1};std::shared_ptr<Shared> state;bool decoded;
public:
    Samples(std::shared_ptr<Shared> shared,bool pixels):state(std::move(shared)),decoded(pixels){}
    HRESULT STDMETHODCALLTYPE QueryInterface(REFIID id,void** value)override {
        if(!value)return E_POINTER;*value=nullptr;
        if(id==IID_IUnknown||id==__uuidof(ISampleGrabberCB)){*value=static_cast<ISampleGrabberCB*>(this);AddRef();return S_OK;}return E_NOINTERFACE;
    }
    ULONG STDMETHODCALLTYPE AddRef()override{return ++references;}
    ULONG STDMETHODCALLTYPE Release()override{auto count=--references;if(!count)delete this;return count;}
    HRESULT STDMETHODCALLTYPE SampleCB(double,IMediaSample* sample)override {
        const long length=sample->GetActualDataLength();if(length<=0||sample->GetSize()<length)return S_OK;
        if(!decoded){++state->input_frames;state->input_bytes+=static_cast<uint64_t>(length);return S_OK;}
        BYTE* bytes=nullptr;if(FAILED(sample->GetPointer(&bytes))||!bytes)return S_OK;
        try{
            std::lock_guard lock(state->mutex);
            if(!state->frame_bytes||static_cast<size_t>(length)<state->frame_bytes)return S_OK;
            if(state->top_down){state->frame.resize(state->frame_bytes);const auto stride=static_cast<size_t>(state->width)*4;
                for(LONG y=0;y<state->height;++y)memcpy(state->frame.data()+static_cast<size_t>(state->height-1-y)*stride,bytes+static_cast<size_t>(y)*stride,stride);
            }else state->frame.assign(bytes,bytes+state->frame_bytes);
            ++state->sequence;++state->decoded_frames;
        }catch(...){return E_OUTOFMEMORY;}
        return S_OK;
    }
    HRESULT STDMETHODCALLTYPE BufferCB(double,BYTE*,long)override{return E_NOTIMPL;}
};
ComPtr<IPin> pin(IBaseFilter* filter,PIN_DIRECTION direction) {
    ComPtr<IEnumPins> list;check(filter->EnumPins(&list));
    for(ComPtr<IPin> current;list->Next(1,&current,nullptr)==S_OK;current.Reset()){
        PIN_DIRECTION actual;check(current->QueryDirection(&actual));if(actual==direction)return current;
    }
    throw Failure{VFW_E_NOT_FOUND};
}
struct Session {
    CaptureSource source;
    std::shared_ptr<Shared> state=std::make_shared<Shared>();
    ComPtr<IBaseFilter> input_filter,decoded_filter,sink;
    ComPtr<ISampleGrabber> input,decoded;
    ComPtr<Samples> input_callback,decoded_callback;
    ComPtr<IMediaControl> control;ComPtr<IMediaEvent> events;
    ComPtr<IAMCameraControl> camera_control;
    long previous_priority=0,previous_priority_flags=0,current_priority=0;
    HRESULT priority_query=E_NOINTERFACE,priority_set=E_NOTIMPL;
    bool priority_changed=false;
    REFERENCE_TIME interval=0;
    std::chrono::steady_clock::time_point started;
    explicit Session(const wchar_t* path):source(path){}
    HRESULT set_low_light(bool enabled)noexcept {
        if(FAILED(priority_query))return priority_query;
        priority_set=camera_control->Set(19,enabled?1:0,CameraControl_Flags_Manual);
        if(SUCCEEDED(priority_set)){priority_changed=true;current_priority=enabled?1:0;}
        return priority_set;
    }
    HRESULT restore_priority()noexcept {
        if(!priority_changed)return S_FALSE;
        auto result=camera_control->Set(19 /* KSPROPERTY_CAMERACONTROL_AUTO_EXPOSURE_PRIORITY */,previous_priority,previous_priority_flags);
        if(SUCCEEDED(result))priority_changed=false;
        return result;
    }
    ~Session(){if(control)control->Stop();if(input)input->SetCallback(nullptr,0);if(decoded)decoded->SetCallback(nullptr,0);restore_priority();}
    void start(int index,const wchar_t* expected_key) {
        int count=0,size=0;check(source.config->GetNumberOfCapabilities(&count,&size));
        if(index<0||index>=count||size<=0||size>65536)throw Failure{E_INVALIDARG};
        std::vector<BYTE> caps(size);MediaType selected;check(source.config->GetStreamCaps(index,&selected.value,caps.data()));
        if(!selected.value||mode_key(*selected.value)!=expected_key||!bounded(video_type(*selected.value)))throw Failure{E_INVALIDARG};
        check(source.config->SetFormat(selected.value));
        MediaType actual;check(source.config->GetFormat(&actual.value));if(!actual.value)throw Failure{E_FAIL};
        interval=video_type(*actual.value).interval;
        // Low-light compensation can silently halve the requested FPS. During a camera
        // test prefer the selected rate; restore the device's original policy on exit.
        if(SUCCEEDED(source.device.As(&camera_control))){
            priority_query=camera_control->Get(19,&previous_priority,&previous_priority_flags);
            current_priority=previous_priority;
            if(SUCCEEDED(priority_query)&&previous_priority!=0){
                set_low_light(false);
            }
        }
        check(CoCreateInstance(grabber_class,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&input_filter)));check(input_filter.As(&input));
        check(CoCreateInstance(grabber_class,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&decoded_filter)));check(decoded_filter.As(&decoded));
        AM_MEDIA_TYPE request{};request.majortype=MEDIATYPE_Video;request.subtype=MEDIASUBTYPE_RGB32;request.formattype=FORMAT_VideoInfo;
        check(decoded->SetMediaType(&request));check(input->SetOneShot(FALSE));check(decoded->SetOneShot(FALSE));
        check(input->SetBufferSamples(FALSE));check(decoded->SetBufferSamples(FALSE));
        input_callback.Attach(new Samples(state,false));decoded_callback.Attach(new Samples(state,true));
        check(input->SetCallback(input_callback.Get(),0));check(decoded->SetCallback(decoded_callback.Get(),0));
        check(source.graph->AddFilter(input_filter.Get(),L"Received frame counter"));check(source.graph->AddFilter(decoded_filter.Get(),L"Latest preview frame"));
        check(CoCreateInstance(null_class,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&sink)));check(source.graph->AddFilter(sink.Get(),L"Preview sink"));
        ComPtr<IPin> capture;check(source.builder->FindPin(source.device.Get(),PINDIR_OUTPUT,&PIN_CATEGORY_CAPTURE,&MEDIATYPE_Video,FALSE,0,&capture));
        check(source.graph->ConnectDirect(capture.Get(),pin(input_filter.Get(),PINDIR_INPUT).Get(),nullptr));
        check(source.graph->Connect(pin(input_filter.Get(),PINDIR_OUTPUT).Get(),pin(decoded_filter.Get(),PINDIR_INPUT).Get()));
        check(source.graph->ConnectDirect(pin(decoded_filter.Get(),PINDIR_OUTPUT).Get(),pin(sink.Get(),PINDIR_INPUT).Get(),nullptr));
        AM_MEDIA_TYPE connected{};check(decoded->GetConnectedMediaType(&connected));
        auto video=video_type(connected);
        if(connected.cbFormat)CoTaskMemFree(connected.pbFormat);if(connected.pUnk)connected.pUnk->Release();
        if(video.width<=0||video.height<=0||static_cast<uint64_t>(video.width)*video.height*4>max_frame_bytes)throw Failure{VFW_E_INVALIDMEDIATYPE};
        {std::lock_guard lock(state->mutex);state->width=video.width;state->height=video.height;state->top_down=video.top_down;state->frame_bytes=static_cast<size_t>(video.width)*video.height*4;}
        check(source.graph.As(&control));check(source.graph.As(&events));
        ComPtr<IMediaFilter> filter;check(source.graph.As(&filter));check(filter->SetSyncSource(nullptr));
        started=std::chrono::steady_clock::now();check(control->Run());
    }
};
template<class F> HRESULT guard(F&& call)noexcept {
    try{call();return S_OK;}catch(const Failure& error){return error.value;}catch(const std::bad_alloc&){return E_OUTOFMEMORY;}catch(...){return E_FAIL;}
}
}
extern "C" {
HRESULT ub_camera_thread_initialize()noexcept {return CoInitializeEx(nullptr,COINIT_MULTITHREADED);}
void ub_camera_thread_uninitialize()noexcept {CoUninitialize();}
struct UbCameraStats { uint64_t frames,bytes,decoded,elapsed_us;int32_t width,height;int64_t interval;int32_t error;uint32_t capture_backend; };
static_assert(sizeof(UbCameraStats)==56);
typedef void (*FrameCallback)(void*,const uint8_t*,size_t,int32_t,int32_t,uint64_t);
HRESULT ub_camera_list(wchar_t** output)noexcept {
    if(!output)return E_POINTER;*output=nullptr;
    return guard([&]{Apartment apartment;std::wstring json=L"[";bool first=true;auto list=monikers();
        if(list)for(ComPtr<IMoniker> moniker;list->Next(1,&moniker,nullptr)==S_OK;moniker.Reset()){
            ComPtr<IPropertyBag> bag;if(FAILED(moniker->BindToStorage(nullptr,nullptr,IID_PPV_ARGS(&bag))))continue;
            auto path=camera_path(moniker.Get(),bag.Get());if(path.empty())continue;
            if(!first)json+=L",";first=false;
            json+=L"{\"path\":"+quote(path)+L",\"name\":"+quote(property(bag.Get(),L"FriendlyName"))+L",\"identities\":"+identities(path)+L"}";
        }
        result_string(json+L"]",output);
    });
}
// Called only for our emulator's verified identity by its elevated receiver.
HRESULT ub_camera_set_name(const wchar_t* path,const wchar_t* name)noexcept {
    if(!path||!name||wcslen(name)>100)return E_INVALIDARG;
    return guard([&]{Apartment apartment;auto list=monikers();bool found=false;
        if(list)for(ComPtr<IMoniker> moniker;list->Next(1,&moniker,nullptr)==S_OK;moniker.Reset()){
            ComPtr<IPropertyBag> bag;if(FAILED(moniker->BindToStorage(nullptr,nullptr,IID_PPV_ARGS(&bag))))continue;
            if(_wcsicmp(camera_path(moniker.Get(),bag.Get()).c_str(),path)!=0)continue;
            PropertyValue value;value.value.vt=VT_BSTR;value.value.bstrVal=SysAllocString(name);if(!value.value.bstrVal)throw std::bad_alloc();
            check(bag->Write(L"FriendlyName",&value.value));found=true;break;
        }
        if(!found)throw Failure{HRESULT_FROM_WIN32(ERROR_NOT_FOUND)};
        DeviceInfoSet owned;SP_DEVICE_INTERFACE_DATA iface{};iface.cbSize=sizeof(iface);
        if(owned.value==INVALID_HANDLE_VALUE||!SetupDiOpenDeviceInterfaceW(owned.value,path,0,&iface))throw Failure{HRESULT_FROM_WIN32(GetLastError())};
        DWORD length=0;SetupDiGetDeviceInterfaceDetailW(owned.value,&iface,nullptr,0,&length,nullptr);
        if(length<sizeof(SP_DEVICE_INTERFACE_DETAIL_DATA_W)||length>65536)throw Failure{E_INVALIDARG};
        std::vector<BYTE> storage(length);auto* detail=reinterpret_cast<SP_DEVICE_INTERFACE_DETAIL_DATA_W*>(storage.data());detail->cbSize=sizeof(*detail);
        SP_DEVINFO_DATA node{};node.cbSize=sizeof(node);
        if(!SetupDiGetDeviceInterfaceDetailW(owned.value,&iface,detail,length,nullptr,&node))throw Failure{HRESULT_FROM_WIN32(GetLastError())};
        if(!SetupDiSetDeviceRegistryPropertyW(owned.value,&node,SPDRP_FRIENDLYNAME,reinterpret_cast<const BYTE*>(name),static_cast<DWORD>((wcslen(name)+1)*sizeof(wchar_t))))throw Failure{HRESULT_FROM_WIN32(GetLastError())};
    });
}
HRESULT ub_camera_modes(const wchar_t* path,wchar_t** output)noexcept {
    if(!path||!output)return E_POINTER;*output=nullptr;
    return guard([&]{Apartment apartment;CaptureSource source(path);int count=0,size=0;check(source.config->GetNumberOfCapabilities(&count,&size));
        if(count<0||count>2048||size<=0||size>65536)throw Failure{E_INVALIDARG};
        std::vector<BYTE> caps(size);std::wstring json=L"[";bool first=true;
        for(int index=0;index<count;++index){MediaType type;if(FAILED(source.config->GetStreamCaps(index,&type.value,caps.data()))||!type.value)continue;
            auto video=video_type(*type.value);if(!bounded(video))continue;
            if(!first)json+=L",";first=false;
            json+=L"{\"index\":"+std::to_wstring(index)+L",\"width\":"+std::to_wstring(video.width)+L",\"height\":"+std::to_wstring(video.height)+L",\"interval\":"+std::to_wstring(video.interval)+L",\"format\":"+quote(subtype_name(type.value->subtype))+L",\"key\":"+quote(mode_key(*type.value))+L"}";
        }
        result_string(json+L"]",output);
    });
}
void ub_camera_string_free(wchar_t* value)noexcept {CoTaskMemFree(value);}
HRESULT ub_camera_open(const wchar_t* path,int32_t index,const wchar_t* key,void** output)noexcept {
    if(!path||!key||!output)return E_POINTER;*output=nullptr;
    HRESULT initialized=CoInitializeEx(nullptr,COINIT_MULTITHREADED);if(FAILED(initialized))return initialized;
    auto result=guard([&]{auto session=std::make_unique<Session>(path);session->start(index,key);*output=session.release();});
    if(FAILED(result))CoUninitialize();return result;
}
HRESULT ub_camera_poll(void* handle,uint64_t* last_sequence,FrameCallback callback,void* context,UbCameraStats* stats)noexcept {
    if(!handle||!last_sequence||!callback||!stats)return E_POINTER;
    return guard([&]{auto& session=*static_cast<Session*>(handle);auto& state=*session.state;
        stats->frames=state.input_frames.load();stats->bytes=state.input_bytes.load();stats->decoded=state.decoded_frames.load();
        stats->elapsed_us=static_cast<uint64_t>(std::chrono::duration_cast<std::chrono::microseconds>(std::chrono::steady_clock::now()-session.started).count());
        stats->width=state.width;stats->height=state.height;stats->interval=session.interval;stats->error=0;
        CLSID source_class{};check(session.source.device->GetClassID(&source_class));
        const CLSID proxy={0x17CCA71B,0xECD7,0x11D0,{0xB9,0x08,0x00,0xA0,0xC9,0x22,0x31,0x96}};
        stats->capture_backend=source_class==proxy?1:2;
        long code=0;LONG_PTR a=0,b=0;
        while(session.events->GetEvent(&code,&a,&b,0)==S_OK){
            if(code==EC_DEVICE_LOST||code==EC_ERRORABORT||code==EC_COMPLETE)stats->error=code==EC_ERRORABORT?static_cast<int32_t>(a):HRESULT_FROM_WIN32(ERROR_DEVICE_NOT_CONNECTED);
            session.events->FreeEventParams(code,a,b);
        }
        std::lock_guard lock(state.mutex);
        if(state.sequence!=*last_sequence&&!state.frame.empty()){
            callback(context,state.frame.data(),state.frame.size(),state.width,state.height,state.sequence);*last_sequence=state.sequence;
        }
    });
}
HRESULT ub_camera_controls(void* handle,wchar_t** output)noexcept {
    if(!handle||!output)return E_POINTER;*output=nullptr;
    return guard([&]{const auto& session=*static_cast<Session*>(handle);
        result_string(L"{\"supported\":"+(SUCCEEDED(session.priority_query)?std::wstring(L"true"):std::wstring(L"false"))+
            L",\"low_light_compensation\":"+(SUCCEEDED(session.priority_query)?(session.current_priority?L"true":L"false"):L"null")+
            L",\"auto_exposure_priority_original\":"+(SUCCEEDED(session.priority_query)?std::to_wstring(session.previous_priority):L"null")+
            L",\"query_hresult\":"+std::to_wstring(static_cast<long>(session.priority_query))+
            L",\"set_hresult\":"+std::to_wstring(static_cast<long>(session.priority_set))+
            L",\"selected_fps_priority_applied\":"+(session.priority_changed?L"true":L"false")+L"}",output);
    });
}
HRESULT ub_camera_set_low_light(void* handle,int32_t enabled)noexcept {
    if(!handle||enabled<0||enabled>1)return E_INVALIDARG;
    return static_cast<Session*>(handle)->set_low_light(enabled!=0);
}
HRESULT ub_camera_close(void* handle)noexcept {if(!handle)return S_FALSE;auto* session=static_cast<Session*>(handle);if(session->control)session->control->Stop();auto restored=session->restore_priority();delete session;CoUninitialize();return restored;}
}
