const first_parcel 5
const parcel_fee 4

parcels weight:Int -> Int
  decreases weight
  ensure result >= 0
  ensure result = 0 when weight <= 0
  ensure result >= 1 when weight > 0
  ensure result <= weight when weight > 0
  ensure result = (weight + 4) / first_parcel when weight > 0
  eg | parcels 0 -> ok
  eg | parcels 5 -> ok
  eg | parcels 6 -> ok
  eg | parcels -3 -> ok
body
  if weight <= 0
    result = 0
  else
    if weight <= first_parcel
      result = 1
    else
      result = 1 + parcels (weight - first_parcel)

shipping weight:Int -> Int
  ensure result = (parcels weight) * parcel_fee
  ensure result >= 0
body
  result = (parcels weight) * parcel_fee
