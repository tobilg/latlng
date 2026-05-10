@0xa1b2c3d4e5f60718;

# Geometry Types

struct Point {
  lat @0 :Float64;
  lon @1 :Float64;
  z   @2 :Float64;
}

struct Bounds {
  minLat @0 :Float64;
  minLon @1 :Float64;
  maxLat @2 :Float64;
  maxLon @3 :Float64;
}

struct GeoObject {
  union {
    point   @0 :Point;
    bounds  @1 :Bounds;
    hash    @2 :Text;
    geojson @3 :Text;
    string  @4 :Text;
  }
}

# Fields

struct FieldEntry {
  name  @0 :Text;
  union {
    number @1 :Float64;
    text   @2 :Text;
    json   @3 :Text;
  }
}

# Search Options

struct WhereFilter {
  field @0 :Text;
  min   @1 :Float64;
  max   @2 :Float64;
}

struct WhereExprFilter {
  expression @0 :Text;
}

enum OutputFormat {
  objects @0;
  points  @1;
  bounds  @2;
  hashes  @3;
  ids     @4;
  count   @5;
}

enum DetectType {
  inside  @0;
  outside @1;
  enter   @2;
  exit    @3;
  cross   @4;
  roam    @5;
}

enum SetCondition {
  always @0;
  nx     @1;
  xx     @2;
}

# Area Specification

struct AreaSpec {
  union {
    circle  :group { lat @0 :Float64; lon @1 :Float64; meters @2 :Float64; }
    bounds  @3 :Bounds;
    hash    @4 :Text;
    object  @5 :Text;
    tile    :group { x @6 :UInt32; y @7 :UInt32; z @8 :UInt32; }
    quadkey @9 :Text;
    sector  :group {
      lat @10 :Float64;
      lon @11 :Float64;
      meters @12 :Float64;
      bearing1 @13 :Float64;
      bearing2 @14 :Float64;
    }
    get     :group { collection @15 :Text; id @16 :Text; }
  }
}

# Request Types

struct SetRequest {
  collection @0 :Text;
  id         @1 :Text;
  object     @2 :GeoObject;
  fields     @3 :List(FieldEntry);
  expireSec  @4 :UInt32;
  condition  @5 :SetCondition;
}

struct GetRequest {
  collection @0 :Text;
  id         @1 :Text;
  withFields @2 :Bool;
  output     @3 :OutputFormat;
  hashPrec   @4 :UInt8;
}

struct SearchOptions {
  cursor     @0 :UInt32;
  limit      @1 :UInt32;
  nofields   @2 :Bool;
  match      @3 :Text;
  asc        @4 :Bool;
  where      @5 :List(WhereFilter);
  whereExpr  @6 :List(WhereExprFilter);
  clip       @7 :Bool;
  output     @8 :OutputFormat;
  hashPrec   @9 :UInt8;
  includeCount @10 :Bool = true;
}

struct NearbyRequest {
  collection @0 :Text;
  lat        @1 :Float64;
  lon        @2 :Float64;
  meters     @3 :Float64;
  options    @4 :SearchOptions;
}

struct WithinRequest {
  collection @0 :Text;
  area       @1 :AreaSpec;
  options    @2 :SearchOptions;
}

struct IntersectsRequest {
  collection @0 :Text;
  area       @1 :AreaSpec;
  options    @2 :SearchOptions;
}

struct ScanRequest {
  collection @0 :Text;
  options    @1 :SearchOptions;
}

struct SearchRequest {
  collection @0 :Text;
  options    @1 :SearchOptions;
}

# Geofence Requests

struct SetChanRequest {
  name @0 :Text;
  search :union {
    nearby     @1 :NearbyRequest;
    within     @2 :WithinRequest;
    intersects @3 :IntersectsRequest;
  }
  detect   @4 :List(DetectType);
  commands @5 :List(Text);
}

struct SetHookRequest {
  name     @0 :Text;
  endpoint @1 :Text;
  search :union {
    nearby     @2 :NearbyRequest;
    within     @3 :WithinRequest;
    intersects @4 :IntersectsRequest;
  }
  detect   @5 :List(DetectType);
  commands @6 :List(Text);
}

struct RoamingGeofenceRequest {
  collection       @0 :Text;
  targetCollection @1 :Text;
  targetPattern    @2 :Text;
  meters           @3 :Float64;
  noDwell          @4 :Bool;
}

# Response Types

struct SearchResult {
  id     @0 :Text;
  object @1 :GeoObject;
  fields @2 :List(FieldEntry);
  dist   @3 :Float64;
}

struct SearchResponse {
  ok      @0 :Bool;
  results @1 :List(SearchResult);
  cursor  @2 :UInt32;
  count   @3 :UInt32;
  error   @4 :Text;
  ids     @5 :List(Text);
}

struct GeofenceEvent {
  command    @0 :Text;
  detect     @1 :DetectType;
  collection @2 :Text;
  id         @3 :Text;
  object     @4 :GeoObject;
  fields     @5 :List(FieldEntry);
  timeNs     @6 :Int64;
  hook       @7 :Text;
  group      @8 :Text;
  nearby :group {
    collection @9  :Text;
    id         @10 :Text;
    meters     @11 :Float64;
  }
}

struct OkResponse {
  ok    @0 :Bool;
  error @1 :Text;
}

struct ServerInfo {
  numCollections @0 :UInt32;
  numObjects     @1 :UInt64;
  numPoints      @2 :UInt64;
  heapBytes      @3 :UInt64;
  readOnly       @4 :Bool;
  leader         @5 :Bool;
  serverId       @6 :Text;
  following      @7 :Text;
  caughtUp       @8 :Bool;
  caughtUpOnce   @9 :Bool;
  lastSequence   @10 :UInt64;
  version        @11 :Text;
  apiVersion     @12 :Text;
  protocolVersion @13 :Text;
  storageFormatVersion @14 :Text;
}

struct ReplicationInfo {
  serverId      @0 :Text;
  following     @1 :Text;
  leader        @2 :Bool;
  lastSequence  @3 :UInt64;
}

struct BoundsResponse {
  ok     @0 :Bool;
  bounds @1 :Bounds;
}

# RPC Interface

interface LatLng {
  set          @0  (req :SetRequest) -> (resp :OkResponse);
  get          @1  (req :GetRequest) -> (result :SearchResult, ok :Bool, error :Text);
  del          @2  (collection :Text, id :Text) -> (resp :OkResponse);
  pdel         @3  (collection :Text, pattern :Text) -> (resp :OkResponse);
  drop         @4  (collection :Text) -> (resp :OkResponse);
  rename       @5  (collection :Text, newname :Text) -> (resp :OkResponse);
  renamenx     @6  (collection :Text, newname :Text) -> (resp :OkResponse);
  fset         @7  (collection :Text, id :Text, fields :List(FieldEntry), xx :Bool) -> (resp :OkResponse);
  fget         @8  (collection :Text, id :Text, field :Text) -> (value :FieldEntry, ok :Bool);
  expire       @9  (collection :Text, id :Text, seconds :UInt32) -> (resp :OkResponse);
  persist      @10 (collection :Text, id :Text) -> (resp :OkResponse);
  ttl          @11 (collection :Text, id :Text) -> (seconds :Int32, ok :Bool);
  exists       @12 (collection :Text, id :Text) -> (exists :Bool);
  fexists      @13 (collection :Text, id :Text, field :Text) -> (exists :Bool);
  bounds       @14 (collection :Text) -> (resp :BoundsResponse);
  collections  @15 (pattern :Text) -> (names :List(Text));
  stats        @16 (collections :List(Text)) -> (stats :List(Text));

  jset         @17 (collection :Text, id :Text, path :Text, value :Text, raw :Bool) -> (resp :OkResponse);
  jget         @18 (collection :Text, id :Text, path :Text) -> (value :Text, ok :Bool);
  jdel         @19 (collection :Text, id :Text, path :Text) -> (resp :OkResponse);

  nearby       @20 (req :NearbyRequest) -> (resp :SearchResponse);
  within       @21 (req :WithinRequest) -> (resp :SearchResponse);
  intersects   @22 (req :IntersectsRequest) -> (resp :SearchResponse);
  scan         @23 (req :ScanRequest) -> (resp :SearchResponse);
  search       @24 (req :SearchRequest) -> (resp :SearchResponse);

  setchan      @25 (req :SetChanRequest) -> (resp :OkResponse);
  delchan      @26 (name :Text) -> (resp :OkResponse);
  pdelchan     @27 (pattern :Text) -> (resp :OkResponse);
  chans        @28 (pattern :Text) -> (channels :List(Text));
  subscribe    @29 (channels :List(Text)) -> (stream :GeofenceStream);
  psubscribe   @30 (patterns :List(Text)) -> (stream :GeofenceStream);
  sethook      @31 (req :SetHookRequest) -> (resp :OkResponse);
  delhook      @32 (name :Text) -> (resp :OkResponse);
  pdelhook     @33 (pattern :Text) -> (resp :OkResponse);
  hooks        @34 (pattern :Text) -> (hooks :List(Text));

  ping         @35 () -> (resp :OkResponse);
  server       @36 () -> (info :ServerInfo);
  info         @37 (section :Text) -> (info :Text);
  healthz      @38 () -> (resp :OkResponse);
  configGet    @39 (name :Text) -> (value :Text);
  configSet    @40 (name :Text, value :Text) -> (resp :OkResponse);
  configRewrite @41 () -> (resp :OkResponse);
  flushdb      @42 () -> (resp :OkResponse);
  gc           @43 () -> (resp :OkResponse);
  readonly     @44 (enabled :Bool) -> (resp :OkResponse);
  auth         @45 (token :Text) -> (resp :OkResponse);
  timeout      @46 (seconds :Float64, command :Text) -> (resp :OkResponse);
  role         @47 () -> (role :Text, info :Text);

  aofshrink    @48 () -> (resp :OkResponse);
  replicationInfo @49 (credential :Text) -> (info :ReplicationInfo, ok :Bool, error :Text);
  replicationChecksum @50 (credential :Text, from :UInt64, to :UInt64) -> (checksum :Data, ok :Bool, error :Text);
  replicationStream @51 (credential :Text, afterSequence :UInt64, batchSize :UInt32) -> (stream :ReplicationStream, ok :Bool, error :Text);
}

interface GeofenceStream {
  next @0 () -> (event :GeofenceEvent, done :Bool);
}

interface ReplicationStream {
  next @0 () -> (entries :List(Data), leaderLastSequence :UInt64);
}
